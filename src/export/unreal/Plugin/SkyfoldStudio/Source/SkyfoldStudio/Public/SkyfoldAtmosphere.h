// Skyfold Studio weather and atmosphere in game, as the editor runs them (`sky::update_weather`):
// the zone under the camera sets the weather (a zone's own weather wins, elsewhere the painted
// biome's), its lighting mood and its haze; everything eases towards them (the weather over a
// couple of seconds, moods over the world's zone fade at borders). The weather drives the
// exposure, how much the sun is dimmed, the sky light, the distance fog and its colour, rain and
// snow round the camera, lightning in storms and the wind. The zone's play rules (flying, mounts,
// rested area) and entry banner are here for the explorer and game code.

#pragma once

#include "CoreMinimal.h"
#include "GameFramework/Actor.h"
#include "SkyfoldAtmosphere.generated.h"

class ADirectionalLight;
class ASkyLight;
class AExponentialHeightFog;
class APostProcessVolume;
class UInstancedStaticMeshComponent;
class UMaterialInterface;
class UStaticMesh;
class FJsonObject;

DECLARE_DYNAMIC_MULTICAST_DELEGATE_ThreeParams(FSkyfoldZoneChanged, int32, Zone, const FString&, Name, const FString&, Banner);

/** The weather mix: each 0..1 (`world::WeatherParams`). */
USTRUCT(BlueprintType)
struct FSkyfoldWeather
{
	GENERATED_BODY()

	UPROPERTY(BlueprintReadOnly, Category = "Skyfold Studio") float Clouds = 0.f;
	UPROPERTY(BlueprintReadOnly, Category = "Skyfold Studio") float Fog = 0.f;
	UPROPERTY(BlueprintReadOnly, Category = "Skyfold Studio") float Wet = 0.f;
	UPROPERTY(BlueprintReadOnly, Category = "Skyfold Studio") float Rain = 0.f;
	UPROPERTY(BlueprintReadOnly, Category = "Skyfold Studio") float Snow = 0.f;
	UPROPERTY(BlueprintReadOnly, Category = "Skyfold Studio") float Dim = 0.f;
	UPROPERTY(BlueprintReadOnly, Category = "Skyfold Studio") float Dust = 0.f;
};

/** A zone's or biome's lighting mood (`world::MoodLight`). */
struct FSkyfoldMood
{
	float Brightness = 0.f;
	float Contrast = 1.f;
	float Saturation = 1.f;
	float Sun = 1.f;
	float ShadowFill = 1.f;
	float Fog = 0.f;
	FLinearColor FogTint = FLinearColor::White;
	float Glow = 0.f;
};

UCLASS(Blueprintable, meta = (DisplayName = "Skyfold Atmosphere"))
class SKYFOLDSTUDIO_API ASkyfoldAtmosphere : public AActor
{
	GENERATED_BODY()

public:
	ASkyfoldAtmosphere();

	/** atmosphere.json from Skyfold Studio (weathers, zones, biomes, the world's weather and wind, fog colours). */
	UPROPERTY(EditAnywhere, Category = "Skyfold Studio|Data", meta = (MultiLine = true))
	FString Data;
	/** Zone and biome ids per terrain vertex (MapSize x MapSize, rows along MapStepJ). */
	UPROPERTY()
	TArray<uint8> ZoneMap;
	UPROPERTY()
	TArray<uint8> BiomeMap;
	UPROPERTY(EditAnywhere, Category = "Skyfold Studio|Data")
	int32 MapSize = 0;
	/** Where map vertex (0, 0) is, and one vertex step along the map's rows and columns (cm). */
	UPROPERTY(EditAnywhere, Category = "Skyfold Studio|Data")
	FVector MapOrigin = FVector::ZeroVector;
	UPROPERTY(EditAnywhere, Category = "Skyfold Studio|Data")
	FVector MapStepI = FVector(100.f, 0.f, 0.f);
	UPROPERTY(EditAnywhere, Category = "Skyfold Studio|Data")
	FVector MapStepJ = FVector(0.f, 100.f, 0.f);

	UPROPERTY(EditAnywhere, Category = "Skyfold Studio|Scene")
	TObjectPtr<ADirectionalLight> Sun;
	UPROPERTY(EditAnywhere, Category = "Skyfold Studio|Scene")
	TObjectPtr<ASkyLight> SkyLight;
	UPROPERTY(EditAnywhere, Category = "Skyfold Studio|Scene")
	TObjectPtr<AExponentialHeightFog> Fog;
	UPROPERTY(EditAnywhere, Category = "Skyfold Studio|Scene")
	TObjectPtr<APostProcessVolume> Look;
	/** Height fog density for 1 / m of the weather's fog (Unreal's density is per 1000 cm). */
	UPROPERTY(EditAnywhere, Category = "Skyfold Studio|Scene")
	float FogUnitsPerMetre = 10.f;
	/** Fog colours are the editor's (sRGB, under the exposure) times this, as in-scattering luminance. */
	UPROPERTY(EditAnywhere, Category = "Skyfold Studio|Scene")
	float FogLuminance = 1.f;

	/** Rain streaks and snow flakes round the camera. */
	UPROPERTY(EditAnywhere, Category = "Skyfold Studio|Weather")
	bool bPrecipitation = true;
	UPROPERTY(EditAnywhere, Category = "Skyfold Studio|Weather")
	TObjectPtr<UStaticMesh> ParticleMesh;
	UPROPERTY(EditAnywhere, Category = "Skyfold Studio|Weather")
	TObjectPtr<UMaterialInterface> RainMaterial;
	UPROPERTY(EditAnywhere, Category = "Skyfold Studio|Weather")
	TObjectPtr<UMaterialInterface> SnowMaterial;
	/** A weather (clear, cloudy, rain, storm, snow, fog, sandstorm) everywhere; empty follows the zones and biomes. */
	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Skyfold Studio|Weather")
	FString WeatherOverride;

	/** Now, at the camera. */
	UPROPERTY(BlueprintReadOnly, Category = "Skyfold Studio") FSkyfoldWeather Weather;
	UPROPERTY(BlueprintReadOnly, Category = "Skyfold Studio") int32 Zone = 0;
	UPROPERTY(BlueprintReadOnly, Category = "Skyfold Studio") int32 Biome = 0;
	UPROPERTY(BlueprintReadOnly, Category = "Skyfold Studio") FString ZoneName;
	/** "Level 10–15  ·  Contested", as the editor's entry banner. */
	UPROPERTY(BlueprintReadOnly, Category = "Skyfold Studio") FString ZoneBanner;
	UPROPERTY(BlueprintReadOnly, Category = "Skyfold Studio") bool bAllowFlying = true;
	UPROPERTY(BlueprintReadOnly, Category = "Skyfold Studio") bool bAllowMounts = true;
	UPROPERTY(BlueprintReadOnly, Category = "Skyfold Studio") bool bRestedArea = false;
	UPROPERTY(BlueprintReadOnly, Category = "Skyfold Studio") float TemperatureC = 15.f;
	/** How wet the ground is (soaks in a minute of steady rain, dries over minutes). */
	UPROPERTY(BlueprintReadOnly, Category = "Skyfold Studio") float Wetness = 0.f;
	/** The wind where the camera is (m/s, and its direction on the ground in Unreal X / Y). */
	UPROPERTY(BlueprintReadOnly, Category = "Skyfold Studio") float WindSpeed = 0.f;
	UPROPERTY(BlueprintReadOnly, Category = "Skyfold Studio") FVector2D WindDirection = FVector2D(1.f, 0.f);
	/** Seconds since the zone last changed (the banner shows for a few). */
	UPROPERTY(BlueprintReadOnly, Category = "Skyfold Studio") float ZoneAge = 100.f;

	UPROPERTY(BlueprintAssignable, Category = "Skyfold Studio")
	FSkyfoldZoneChanged OnZoneChanged;

	UFUNCTION(BlueprintPure, Category = "Skyfold Studio", meta = (WorldContext = "WorldContext"))
	static ASkyfoldAtmosphere* Get(const UObject* WorldContext);

	/** The zone and biome ids under a world position. */
	UFUNCTION(BlueprintPure, Category = "Skyfold Studio")
	void IdsAt(const FVector& Location, int32& OutZone, int32& OutBiome) const;

	virtual void BeginPlay() override;
	virtual void Tick(float DeltaSeconds) override;

private:
	TSharedPtr<FJsonObject> Json;
	FSkyfoldMood Mood;
	float Haze = 1.f;
	float HazeAmount = 0.f;
	bool bFirst = true;
	float Flash = 0.f;
	float StrikeIn = 8.f;
	float BaseBias = 0.f;
	float BaseSun = 1.f;
	float BaseSky = 1.f;
	float BaseFog = 0.f;
	FVector4 BaseSaturation = FVector4(1, 1, 1, 1);
	FVector4 BaseContrast = FVector4(1, 1, 1, 1);
	FLinearColor BaseFogColor = FLinearColor::Gray;
	FVector2D Drift = FVector2D::ZeroVector;

	UPROPERTY(Transient)
	TObjectPtr<UInstancedStaticMeshComponent> RainDrops;
	UPROPERTY(Transient)
	TObjectPtr<UInstancedStaticMeshComponent> SnowFlakes;
	TArray<FVector> RainPos;
	TArray<FVector> SnowPos;

	FSkyfoldWeather Params(const FString& Name, float K) const;
	void Target(int32 InZone, int32 InBiome, FSkyfoldWeather& OutWeather, FSkyfoldMood& OutMood, float& OutHaze, float& OutAmount) const;
	void Apply(const FVector& Camera, float DeltaSeconds);
	void Precipitate(UInstancedStaticMeshComponent* Comp, TArray<FVector>& Pos, int32 Max, float Amount, const FVector& Camera, const FVector& Box, float Fall, float DeltaSeconds, float Size, float Length);
	const FJsonObject* Section(const TCHAR* Key) const;
};
