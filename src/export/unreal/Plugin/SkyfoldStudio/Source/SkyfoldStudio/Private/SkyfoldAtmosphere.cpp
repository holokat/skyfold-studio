#include "SkyfoldAtmosphere.h"

#include "Components/DirectionalLightComponent.h"
#include "Components/ExponentialHeightFogComponent.h"
#include "Components/InstancedStaticMeshComponent.h"
#include "Components/SkyLightComponent.h"
#include "Dom/JsonObject.h"
#include "Engine/DirectionalLight.h"
#include "Engine/ExponentialHeightFog.h"
#include "Engine/PostProcessVolume.h"
#include "Engine/SkyLight.h"
#include "Engine/World.h"
#include "EngineUtils.h"
#include "Kismet/GameplayStatics.h"
#include "Camera/PlayerCameraManager.h"
#include "Serialization/JsonReader.h"
#include "Serialization/JsonSerializer.h"

namespace
{
	float Num(const FJsonObject* O, const TCHAR* Key, float Default = 0.f)
	{
		double V = Default;
		return O && O->TryGetNumberField(Key, V) ? float(V) : Default;
	}

	bool Flag(const FJsonObject* O, const TCHAR* Key, bool Default = false)
	{
		bool V = Default;
		return O && O->TryGetBoolField(Key, V) ? V : Default;
	}

	FString Str(const FJsonObject* O, const TCHAR* Key)
	{
		FString V;
		return O && O->TryGetStringField(Key, V) ? V : FString();
	}

	const FJsonObject* Obj(const FJsonObject* O, const FString& Key)
	{
		const TSharedPtr<FJsonObject>* V = nullptr;
		return O && O->TryGetObjectField(Key, V) && V && V->IsValid() ? V->Get() : nullptr;
	}

	FLinearColor Rgb(const FJsonObject* O, const TCHAR* Key, FLinearColor Default)
	{
		const TArray<TSharedPtr<FJsonValue>>* A = nullptr;
		if (O && O->TryGetArrayField(Key, A) && A->Num() >= 3)
		{
			return FLinearColor(float((*A)[0]->AsNumber()), float((*A)[1]->AsNumber()), float((*A)[2]->AsNumber()));
		}
		return Default;
	}

	/** sRGB (as the editor writes its colours) to linear. */
	FLinearColor Lin(const FLinearColor& C)
	{
		return FLinearColor::FromSRGBColor(C.ToFColor(false));
	}

	float Ease(float Delta, float Rate)
	{
		return 1.f - FMath::Exp(-Delta * Rate);
	}
}

ASkyfoldAtmosphere::ASkyfoldAtmosphere()
{
	PrimaryActorTick.bCanEverTick = true;
	PrimaryActorTick.TickGroup = TG_PostUpdateWork;
	RootComponent = CreateDefaultSubobject<USceneComponent>(TEXT("Root"));
}

ASkyfoldAtmosphere* ASkyfoldAtmosphere::Get(const UObject* WorldContext)
{
	UWorld* World = GEngine ? GEngine->GetWorldFromContextObject(WorldContext, EGetWorldErrorMode::ReturnNull) : nullptr;
	if (World)
	{
		TActorIterator<ASkyfoldAtmosphere> It(World);
		if (It)
		{
			return *It;
		}
	}
	return nullptr;
}

const FJsonObject* ASkyfoldAtmosphere::Section(const TCHAR* Key) const
{
	return Obj(Json.Get(), Key);
}

void ASkyfoldAtmosphere::IdsAt(const FVector& Location, int32& OutZone, int32& OutBiome) const
{
	OutZone = 0;
	OutBiome = 0;
	if (MapSize < 2)
	{
		return;
	}
	// solve Location - Origin = i * StepI + j * StepJ on the ground
	const FVector2D D(Location.X - MapOrigin.X, Location.Y - MapOrigin.Y);
	const double A = MapStepI.X, B = MapStepJ.X, C = MapStepI.Y, E = MapStepJ.Y;
	const double Det = A * E - B * C;
	if (FMath::Abs(Det) < 1e-6)
	{
		return;
	}
	const int32 I = FMath::RoundToInt((D.X * E - B * D.Y) / Det);
	const int32 J = FMath::RoundToInt((A * D.Y - C * D.X) / Det);
	if (I < 0 || J < 0 || I >= MapSize || J >= MapSize)
	{
		return;
	}
	const int32 K = J * MapSize + I;
	OutZone = ZoneMap.IsValidIndex(K) ? ZoneMap[K] : 0;
	OutBiome = BiomeMap.IsValidIndex(K) ? BiomeMap[K] : 0;
}

void ASkyfoldAtmosphere::BeginPlay()
{
	Super::BeginPlay();
	TSharedRef<TJsonReader<>> Reader = TJsonReaderFactory<>::Create(Data);
	if (!FJsonSerializer::Deserialize(Reader, Json) || !Json.IsValid())
	{
		UE_LOG(LogTemp, Warning, TEXT("Skyfold Studio: the atmosphere has no data"));
		Json.Reset();
		return;
	}
	if (Look)
	{
		BaseBias = Look->Settings.AutoExposureBias;
		BaseSaturation = Look->Settings.ColorSaturation;
		BaseContrast = Look->Settings.ColorContrast;
		Look->Settings.bOverride_ColorContrast = true;
		Look->Settings.bOverride_ColorSaturation = true;
		Look->Settings.bOverride_AutoExposureBias = true;
	}
	if (Sun && Sun->GetLightComponent())
	{
		BaseSun = Sun->GetLightComponent()->Intensity;
	}
	if (SkyLight && SkyLight->GetLightComponent())
	{
		BaseSky = SkyLight->GetLightComponent()->Intensity;
	}
	if (Fog && Fog->GetComponent())
	{
		BaseFog = Fog->GetComponent()->FogDensity;
		BaseFogColor = Fog->GetComponent()->FogInscatteringLuminance;
	}
	if (bPrecipitation && ParticleMesh)
	{
		auto Make = [this](const TCHAR* Name, UMaterialInterface* Material) {
			UInstancedStaticMeshComponent* C = NewObject<UInstancedStaticMeshComponent>(this, Name);
			C->SetStaticMesh(ParticleMesh);
			if (Material)
			{
				C->SetMaterial(0, Material);
			}
			C->SetCollisionEnabled(ECollisionEnabled::NoCollision);
			C->SetCastShadow(false);
			C->SetUsingAbsoluteLocation(true);
			C->SetUsingAbsoluteRotation(true);
			C->SetUsingAbsoluteScale(true);
			C->SetupAttachment(RootComponent);
			C->RegisterComponent();
			return C;
		};
		RainDrops = Make(TEXT("Rain"), RainMaterial);
		SnowFlakes = Make(TEXT("Snow"), SnowMaterial);
	}
}

FSkyfoldWeather ASkyfoldAtmosphere::Params(const FString& Name, float K) const
{
	const FJsonObject* All = Section(TEXT("weathers"));
	const FJsonObject* Clear = Obj(All, TEXT("clear"));
	const FString Wanted = Name.ToLower();
	const FJsonObject* W = Obj(All, Wanted);
	if (W == nullptr && All)
	{
		for (const auto& P : All->Values)
		{
			const FJsonObject* C = P.Value.IsValid() ? P.Value->AsObject().Get() : nullptr;
			if (Str(C, TEXT("name")).ToLower() == Wanted)
			{
				W = C;
			}
		}
	}
	if (W == nullptr)
	{
		W = Clear;
	}
	K = FMath::Clamp(K, 0.f, 1.f);
	auto L = [&](const TCHAR* Key) { return FMath::Lerp(Num(Clear, Key), Num(W, Key), K); };
	FSkyfoldWeather Out;
	Out.Clouds = L(TEXT("clouds"));
	Out.Fog = L(TEXT("fog"));
	Out.Wet = L(TEXT("wet"));
	Out.Rain = L(TEXT("rain"));
	Out.Snow = L(TEXT("snow"));
	Out.Dim = L(TEXT("dim"));
	Out.Dust = L(TEXT("dust"));
	return Out;
}

void ASkyfoldAtmosphere::Target(int32 InZone, int32 InBiome, FSkyfoldWeather& OutWeather, FSkyfoldMood& OutMood, float& OutHaze, float& OutAmount) const
{
	const FJsonObject* World = Section(TEXT("world"));
	const FJsonObject* ZoneData = Obj(Section(TEXT("zones")), FString::FromInt(InZone));
	const FJsonObject* BiomeData = Obj(Section(TEXT("biomes")), FString::FromInt(InBiome));
	const float WorldClouds = Num(World, TEXT("clouds"));
	OutWeather = Params(TEXT("clear"), 1.f);
	OutWeather.Clouds = WorldClouds;
	FString Over = WeatherOverride.IsEmpty() ? Str(World, TEXT("override")) : WeatherOverride;
	if (!Over.IsEmpty())
	{
		OutWeather = Params(Over, 1.f);
		OutWeather.Clouds = FMath::Max(OutWeather.Clouds, WorldClouds);
	}
	else
	{
		const bool bZoneWeather = Flag(ZoneData, TEXT("weather_enabled"));
		if (bZoneWeather)
		{
			OutWeather = Params(Str(ZoneData, TEXT("weather")), Num(ZoneData, TEXT("intensity"), 1.f));
			OutWeather.Clouds = FMath::Max(OutWeather.Clouds, WorldClouds);
		}
		else if (BiomeData)
		{
			OutWeather = Params(Str(BiomeData, TEXT("weather")), Num(BiomeData, TEXT("intensity"), 1.f));
			OutWeather.Clouds = FMath::Max(OutWeather.Clouds, WorldClouds);
		}
		if (Flag(ZoneData, TEXT("clouds_enabled")))
		{
			const float Cover = Num(ZoneData, TEXT("cloud_cover"));
			OutWeather.Clouds = bZoneWeather ? FMath::Max(OutWeather.Clouds, Cover) : Cover;
		}
	}
	if (!Flag(World, TEXT("precipitation"), true))
	{
		OutWeather.Rain = OutWeather.Snow = OutWeather.Wet = 0.f;
	}
	// the zone's lighting mood, else the biome's
	OutMood = FSkyfoldMood();
	const FJsonObject* Light = Obj(ZoneData, TEXT("light"));
	if (Light == nullptr)
	{
		Light = Obj(BiomeData, TEXT("light"));
	}
	if (Light)
	{
		OutMood.Brightness = Num(Light, TEXT("brightness"));
		OutMood.Contrast = Num(Light, TEXT("contrast"), 1.f);
		OutMood.Saturation = Num(Light, TEXT("saturation"), 1.f);
		OutMood.Sun = Num(Light, TEXT("sun"), 1.f);
		OutMood.ShadowFill = Num(Light, TEXT("shadow_fill"), 1.f);
		OutMood.Fog = Num(Light, TEXT("fog"));
		OutMood.FogTint = Rgb(Light, TEXT("fog_tint"), FLinearColor::White);
		OutMood.Glow = Num(Light, TEXT("glow"));
	}
	// the zone's haze override
	OutHaze = Flag(World, TEXT("haze_enabled"), true) ? 1.f : 0.f;
	OutAmount = Num(World, TEXT("haze"));
	const FString Mode = Str(ZoneData, TEXT("haze_mode"));
	if (Mode == TEXT("on"))
	{
		OutHaze = 1.f;
		OutAmount = Num(ZoneData, TEXT("haze_amount"), OutAmount);
	}
	else if (Mode == TEXT("off"))
	{
		OutHaze = 0.f;
	}
}

void ASkyfoldAtmosphere::Tick(float DeltaSeconds)
{
	Super::Tick(DeltaSeconds);
	if (!Json.IsValid())
	{
		return;
	}
	APlayerCameraManager* Cam = UGameplayStatics::GetPlayerCameraManager(this, 0);
	const FVector Camera = Cam ? Cam->GetCameraLocation() : GetActorLocation();
	int32 NewZone = 0, NewBiome = 0;
	IdsAt(Camera, NewZone, NewBiome);
	Biome = NewBiome;
	ZoneAge += DeltaSeconds;
	if (NewZone != Zone || bFirst)
	{
		Zone = NewZone;
		const FJsonObject* ZoneData = Obj(Section(TEXT("zones")), FString::FromInt(Zone));
		ZoneName = Str(ZoneData, TEXT("name"));
		ZoneBanner = Str(ZoneData, TEXT("banner"));
		bAllowFlying = Flag(ZoneData, TEXT("allow_flying"), true);
		bAllowMounts = Flag(ZoneData, TEXT("allow_mounts"), true);
		bRestedArea = Flag(ZoneData, TEXT("rested_area"));
		TemperatureC = Num(ZoneData, TEXT("temperature_c"), 15.f);
		if (!bFirst && ZoneData)
		{
			ZoneAge = 0.f;
			OnZoneChanged.Broadcast(Zone, ZoneName, ZoneBanner);
		}
	}
	FSkyfoldWeather TW;
	FSkyfoldMood TM;
	float TH = 1.f, TA = 0.f;
	Target(Zone, Biome, TW, TM, TH, TA);
	const float K = bFirst ? 1.f : Ease(DeltaSeconds, 0.6f);
	const float KL = bFirst ? 1.f : Ease(DeltaSeconds, 3.f / FMath::Max(Num(Json.Get(), TEXT("fade_seconds"), 3.f), 0.1f));
	Weather.Clouds = FMath::Lerp(Weather.Clouds, TW.Clouds, K);
	Weather.Fog = FMath::Lerp(Weather.Fog, TW.Fog, K);
	Weather.Wet = FMath::Lerp(Weather.Wet, TW.Wet, K);
	Weather.Rain = FMath::Lerp(Weather.Rain, TW.Rain, K);
	Weather.Snow = FMath::Lerp(Weather.Snow, TW.Snow, K);
	Weather.Dim = FMath::Lerp(Weather.Dim, TW.Dim, K);
	Weather.Dust = FMath::Lerp(Weather.Dust, TW.Dust, K);
	Mood.Brightness = FMath::Lerp(Mood.Brightness, TM.Brightness, KL);
	Mood.Contrast = FMath::Lerp(Mood.Contrast, TM.Contrast, KL);
	Mood.Saturation = FMath::Lerp(Mood.Saturation, TM.Saturation, KL);
	Mood.Sun = FMath::Lerp(Mood.Sun, TM.Sun, KL);
	Mood.ShadowFill = FMath::Lerp(Mood.ShadowFill, TM.ShadowFill, KL);
	Mood.Fog = FMath::Lerp(Mood.Fog, TM.Fog, KL);
	Mood.FogTint = FMath::Lerp(Mood.FogTint, TM.FogTint, KL);
	Mood.Glow = FMath::Lerp(Mood.Glow, TM.Glow, KL);
	Haze = FMath::Lerp(Haze, TH, KL);
	HazeAmount = TA;

	// wind: the world's or the zone's, stronger in storms and dust (`wind::update_wind`)
	const FJsonObject* World = Section(TEXT("world"));
	const FJsonObject* Wind = Obj(World, TEXT("wind"));
	const FJsonObject* ZoneData = Obj(Section(TEXT("zones")), FString::FromInt(Zone));
	float Speed = Flag(ZoneData, TEXT("wind_enabled")) ? Num(ZoneData, TEXT("wind_speed")) : Num(Wind, TEXT("speed"));
	Speed = Speed * (1.f + Weather.Rain * 0.6f) + Weather.Dim * 5.f * Weather.Rain + Weather.Dust * 9.f;
	WindSpeed = bFirst ? Speed : FMath::Lerp(WindSpeed, Speed, Ease(DeltaSeconds, 0.8f));
	{
		const FLinearColor D = Rgb(Wind, TEXT("dir"), FLinearColor(1, 0, 0));
		// (editor x, z on the ground) -> Unreal, through the map's own axes
		const FVector G = MapStepI.GetSafeNormal() * D.R + MapStepJ.GetSafeNormal() * D.G;
		const FVector2D T = FVector2D(G.X, G.Y).GetSafeNormal();
		if (!T.IsNearlyZero())
		{
			WindDirection = T;
		}
	}

	// wet ground (the editor's time constants: soaks in a minute of steady rain, dries in minutes)
	const float RainNow = FMath::Clamp(Weather.Rain, 0.f, 1.f);
	const float WetTo = FMath::Clamp(Weather.Wet, 0.f, 1.f);
	const float Tau = bFirst ? 0.4f : (WetTo > Wetness ? 18.f / (0.25f + RainNow) : 150.f);
	Wetness += (WetTo - Wetness) * (1.f - FMath::Exp(-DeltaSeconds / Tau));

	// lightning in storms
	Flash = FMath::Max(Flash - DeltaSeconds * 6.f, 0.f);
	const float Storm = FMath::Clamp((Weather.Rain - 0.6f) * 2.5f, 0.f, 1.f) * FMath::Clamp((Weather.Dim - 0.55f) * 4.f, 0.f, 1.f);
	const float Rate = Num(World, TEXT("lightning"), 1.f) * Storm * 3.f / 60.f;
	if (Rate > 0.f)
	{
		StrikeIn -= DeltaSeconds;
		if (StrikeIn <= 0.f)
		{
			Flash = 1.f;
			StrikeIn = FMath::FRandRange(0.3f, 2.f) / Rate;
		}
	}
	bFirst = false;
	Apply(Camera, DeltaSeconds);
}

void ASkyfoldAtmosphere::Apply(const FVector& Camera, float DeltaSeconds)
{
	const FSkyfoldWeather& W = Weather;
	const float Dim = W.Dim;
	const FJsonObject* LookData = Section(TEXT("look"));
	const FJsonObject* FogData = Section(TEXT("fog"));
	if (Look)
	{
		// the eye opens under heavy cloud; the mood's brightness, contrast and saturation
		Look->Settings.AutoExposureBias = BaseBias + 1.3f * Dim + Mood.Brightness;
		Look->Settings.ColorSaturation = BaseSaturation * Mood.Saturation;
		Look->Settings.ColorContrast = BaseContrast * Mood.Contrast;
	}
	if (Sun && Sun->GetLightComponent())
	{
		Sun->GetLightComponent()->SetIntensity(BaseSun * (1.f - 0.85f * Dim) * Mood.Sun + Flash * 2.5f * BaseSun);
	}
	if (SkyLight && SkyLight->GetLightComponent())
	{
		SkyLight->GetLightComponent()->SetIntensity(BaseSky * Mood.ShadowFill * (1.f + Flash * 1.5f));
	}
	if (Fog && Fog->GetComponent())
	{
		// clear haze (as imported) where the zone keeps it, plus the weather's and the mood's fog
		const float S = Haze;
		const float LH = Num(LookData, TEXT("haze"), 1.f);
		const float Near = (W.Fog * 0.0016f + W.Rain * 0.0004f + W.Snow * 0.0006f + Mood.Fog * Num(FogData, TEXT("zone_density"))) * 0.25f * S;
		const float Air = (W.Fog * 0.007f + Mood.Fog * 0.005f) * LH * S + (W.Rain * 0.0005f + W.Snow * 0.0009f + W.Dust * 0.004f) * S;
		const float Extra = (Near + Air) * FogUnitsPerMetre;
		const float Clear = BaseFog * S;
		Fog->GetComponent()->SetFogDensity(Clear + Extra);
		// its colour: clear haze, greyer under storm cloud, dusty in sandstorms, sunlit grey weather fog
		FLinearColor Col = FMath::Lerp(Lin(Rgb(FogData, TEXT("clear_color"), FLinearColor::Gray)), Lin(Rgb(FogData, TEXT("storm_color"), FLinearColor::Gray)), Dim * 0.7f);
		Col = FMath::Lerp(Col, Lin(Rgb(FogData, TEXT("dust_color"), FLinearColor::Gray)), W.Dust);
		const float Share = FMath::Clamp(Extra / FMath::Max(Clear + Extra, 1e-6f), 0.f, 1.f);
		Col = FMath::Lerp(Col, Lin(Rgb(FogData, TEXT("weather_color"), FLinearColor::Gray)), Share * (1.f - W.Dust));
		Col *= Mood.FogTint * FogLuminance;
		Col.A = 1.f;
		Fog->GetComponent()->SetFogInscatteringColor(Col);
	}
	// rain streaks and snow flakes round the camera
	Precipitate(RainDrops, RainPos, 6000, W.Rain, Camera, FVector(3500, 3500, 1200), 1100.f, DeltaSeconds, 1.2f, 90.f);
	Precipitate(SnowFlakes, SnowPos, 9000, W.Snow, Camera, FVector(2500, 2500, 1200), 120.f, DeltaSeconds, 9.f, 9.f);
}

void ASkyfoldAtmosphere::Precipitate(UInstancedStaticMeshComponent* Comp, TArray<FVector>& Pos, int32 Max, float Amount, const FVector& Camera, const FVector& Box, float Fall, float DeltaSeconds, float Size, float Length)
{
	if (Comp == nullptr)
	{
		return;
	}
	const int32 Want = FMath::Clamp(int32(Max * FMath::Clamp(Amount, 0.f, 1.f)), 0, Max);
	if (Want == 0)
	{
		if (Comp->GetInstanceCount() > 0)
		{
			Comp->ClearInstances();
			Pos.Reset();
		}
		return;
	}
	while (Pos.Num() < Want)
	{
		Pos.Add(Camera + FVector(FMath::FRandRange(-Box.X, Box.X), FMath::FRandRange(-Box.Y, Box.Y), FMath::FRandRange(-Box.Z, Box.Z)));
	}
	Pos.SetNum(Want);
	// falling, blown by the wind, wrapped round the camera
	const FVector Vel = FVector(WindDirection.X * WindSpeed * 60.f, WindDirection.Y * WindSpeed * 60.f, -Fall);
	const FVector Step = Vel * DeltaSeconds;
	// streaks stand up and face the camera (the plane mesh is 100 cm square, facing +Z: its X
	// becomes the vertical, its normal the way to the camera)
	TArray<FTransform> Xf;
	Xf.SetNum(Want);
	for (int32 i = 0; i < Want; ++i)
	{
		FVector P = Pos[i] + Step;
		FVector L = P - Camera;
		for (int32 A = 0; A < 3; ++A)
		{
			const double Half = Box[A];
			if (L[A] < -Half) { L[A] += 2 * Half; }
			if (L[A] > Half) { L[A] -= 2 * Half; }
		}
		P = Camera + L;
		Pos[i] = P;
		const FVector To = (Camera - P).GetSafeNormal2D();
		const FRotator R = FRotationMatrix::MakeFromXZ(FVector::UpVector, To).Rotator();
		Xf[i] = FTransform(R, P, FVector(Length / 100.f, Size / 100.f, 1.f));
	}
	if (Comp->GetInstanceCount() != Want)
	{
		Comp->ClearInstances();
		Comp->AddInstances(Xf, false, true);
	}
	else
	{
		Comp->BatchUpdateInstancesTransforms(0, Xf, true, true, true);
	}
}
