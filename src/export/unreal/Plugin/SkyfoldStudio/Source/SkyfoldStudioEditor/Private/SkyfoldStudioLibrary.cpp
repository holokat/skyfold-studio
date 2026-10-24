#include "SkyfoldStudioLibrary.h"

#include "Editor.h"
#include "Engine/World.h"
#include "FileHelpers.h"
#include "Landscape.h"
#include "LandscapeComponent.h"
#include "LandscapeInfo.h"
#include "LandscapeLayerInfoObject.h"
#include "LandscapeProxy.h"
#include "LandscapeUtils.h"
#include "Materials/MaterialInterface.h"
#include "Misc/FileHelper.h"
#include "SkyfoldAtmosphere.h"
#include "UObject/Package.h"

DEFINE_LOG_CATEGORY_STATIC(LogSkyfoldStudio, Log, All);

namespace
{
	constexpr int32 QuadsPerSection = 63;
	constexpr int32 SectionsPerComponent = 2;

	bool LoadRaw(const FString& Path, int64 Expected, TArray<uint8>& Out)
	{
		if (!FFileHelper::LoadFileToArray(Out, *Path))
		{
			UE_LOG(LogSkyfoldStudio, Error, TEXT("Skyfold Studio: can't read %s"), *Path);
			return false;
		}
		if (Out.Num() != Expected)
		{
			UE_LOG(LogSkyfoldStudio, Error, TEXT("Skyfold Studio: %s has %d bytes, expected %lld"), *Path, Out.Num(), Expected);
			return false;
		}
		return true;
	}
}

ALandscape* USkyfoldStudioLibrary::CreateLandscape(const FString& HeightFile, int32 Resolution, FVector Location, FVector Scale,
	UMaterialInterface* Material, const TArray<FString>& LayerNames, const TArray<FString>& LayerFiles,
	const FString& LayerInfoFolder, const FString& HoleFile, const FString& Label)
{
	const int32 QuadsPerComponent = QuadsPerSection * SectionsPerComponent;
	if (Resolution < QuadsPerComponent + 1 || (Resolution - 1) % QuadsPerComponent != 0)
	{
		UE_LOG(LogSkyfoldStudio, Error, TEXT("Skyfold Studio: landscape resolution %d isn't 126 x k + 1"), Resolution);
		return nullptr;
	}
	UWorld* World = GEditor ? GEditor->GetEditorWorldContext().World() : nullptr;
	if (World == nullptr)
	{
		UE_LOG(LogSkyfoldStudio, Error, TEXT("Skyfold Studio: no editor world"));
		return nullptr;
	}
	const int64 Samples = int64(Resolution) * Resolution;
	TArray<uint8> Bytes;
	if (!LoadRaw(HeightFile, Samples * 2, Bytes))
	{
		return nullptr;
	}
	TArray<uint16> Heights;
	Heights.SetNumUninitialized(Samples);
	FMemory::Memcpy(Heights.GetData(), Bytes.GetData(), Samples * 2);

	// the target layers: weight-blended, a layer info asset each
	TArray<FLandscapeImportLayerInfo> Layers;
	for (int32 i = 0; i < LayerNames.Num(); ++i)
	{
		FLandscapeImportLayerInfo Layer{FName(*LayerNames[i])};
		ULandscapeLayerInfoObject* Info = UE::Landscape::CreateTargetLayerInfo(Layer.LayerName, LayerInfoFolder, TEXT("LI_") + LayerNames[i]);
		Info->SetBlendMethod(ELandscapeTargetLayerBlendMethod::FinalWeightBlending, false);
		Layer.LayerInfo = Info;
		if (LayerFiles.IsValidIndex(i) && !LoadRaw(LayerFiles[i], Samples, Layer.LayerData))
		{
			Layer.LayerData.Init(i == 0 ? 255 : 0, Samples);
		}
		Layers.Add(MoveTemp(Layer));
	}
	// the cave mouths: the visibility layer
	bool bHoles = false;
	if (!HoleFile.IsEmpty() && ALandscapeProxy::VisibilityLayer != nullptr)
	{
		FLandscapeImportLayerInfo Holes(ALandscapeProxy::VisibilityLayer->GetLayerName());
		Holes.LayerInfo = ALandscapeProxy::VisibilityLayer;
		if (LoadRaw(HoleFile, Samples, Holes.LayerData))
		{
			Layers.Add(MoveTemp(Holes));
			bHoles = true;
		}
	}

	TMap<FGuid, TArray<uint16>> HeightData;
	HeightData.Add(FGuid(), MoveTemp(Heights));
	TMap<FGuid, TArray<FLandscapeImportLayerInfo>> LayerData;
	LayerData.Add(FGuid(), Layers);

	FActorSpawnParameters Params;
	ALandscape* Landscape = World->SpawnActor<ALandscape>(Location, FRotator::ZeroRotator, Params);
	if (Landscape == nullptr)
	{
		return nullptr;
	}
	Landscape->LandscapeMaterial = Material;
	Landscape->SetActorRelativeScale3D(Scale);
	Landscape->StaticLightingLOD = FMath::DivideAndRoundUp(FMath::CeilLogTwo((Resolution * Resolution) / (2048 * 2048) + 1), (uint32)2);
	Landscape->Import(FGuid::NewGuid(), 0, 0, Resolution - 1, Resolution - 1, SectionsPerComponent, QuadsPerSection, HeightData,
		nullptr, LayerData, ELandscapeImportAlphamapType::Additive, TArrayView<const FLandscapeLayer>());
	if (!Label.IsEmpty())
	{
		Landscape->SetActorLabel(Label);
	}
	ULandscapeInfo* Info = Landscape->GetLandscapeInfo();
	if (Info != nullptr)
	{
		Info->UpdateLayerInfoMap(Landscape);
		for (const FLandscapeImportLayerInfo& Layer : Layers)
		{
			if (Layer.LayerInfo == ALandscapeProxy::VisibilityLayer)
			{
				continue;
			}
			Landscape->AddTargetLayer(Layer.LayerName, FLandscapeTargetLayerSettings(Layer.LayerInfo), false);
			const int32 Index = Info->GetLayerInfoIndex(Layer.LayerName);
			if (Index != INDEX_NONE)
			{
				Info->Layers[Index].LayerInfoObj = Layer.LayerInfo;
			}
		}
	}
	// the layer infos are assets of their own: saved now, so the level never refers to unsaved ones
	TArray<UPackage*> Packages;
	for (const FLandscapeImportLayerInfo& Layer : Layers)
	{
		if (Layer.LayerInfo && Layer.LayerInfo != ALandscapeProxy::VisibilityLayer)
		{
			Packages.Add(Layer.LayerInfo->GetPackage());
		}
	}
	UEditorLoadingAndSavingUtils::SavePackages(Packages, false);
	Landscape->PostEditChange();
	// edit layers: Import writes the default edit layer and only requests the GPU merge into the
	// final heightmaps and weightmaps; run it now, so a save straight after keeps the paint
	if (FApp::CanEverRender())
	{
		Landscape->ForceUpdateLayersContent();
	}
	else
	{
		UE_LOG(LogSkyfoldStudio, Warning, TEXT("Skyfold Studio: no renderer (-nullrhi): the landscape's layers merge when the level is next opened in the editor"));
	}
	UE_LOG(LogSkyfoldStudio, Display, TEXT("Skyfold Studio: landscape %d x %d, %d layers%s"), Resolution, Resolution, LayerNames.Num(), bHoles ? TEXT(", cave holes") : TEXT(""));
	return Landscape;
}

FString USkyfoldStudioLibrary::DescribeLandscape(ALandscapeProxy* Landscape)
{
	if (Landscape == nullptr)
	{
		return TEXT("no landscape");
	}
	TMap<FName, int32> Painted;
	for (ULandscapeComponent* Component : Landscape->LandscapeComponents)
	{
		for (const FWeightmapLayerAllocationInfo& Allocation : Component->GetWeightmapLayerAllocations())
		{
			Painted.FindOrAdd(Allocation.LayerInfo ? Allocation.LayerInfo->GetLayerName() : FName(TEXT("?")))++;
		}
	}
	FString Out = FString::Printf(TEXT("%d components;"), Landscape->LandscapeComponents.Num());
	for (const TPair<FName, int32>& P : Painted)
	{
		Out += FString::Printf(TEXT(" %s %d"), *P.Key.ToString(), P.Value);
	}
	return Out;
}

bool USkyfoldStudioLibrary::LoadAtmosphere(ASkyfoldAtmosphere* Atmosphere, const FString& JsonFile, const FString& ZonesFile, const FString& BiomesFile, int32 Size)
{
	if (Atmosphere == nullptr || Size < 2)
	{
		return false;
	}
	FString Data;
	if (!FFileHelper::LoadFileToString(Data, *JsonFile))
	{
		UE_LOG(LogSkyfoldStudio, Error, TEXT("Skyfold Studio: can't read %s"), *JsonFile);
		return false;
	}
	TArray<uint8> Zones, Biomes;
	const int64 Samples = int64(Size) * Size;
	if (!LoadRaw(ZonesFile, Samples, Zones) || !LoadRaw(BiomesFile, Samples, Biomes))
	{
		return false;
	}
	Atmosphere->Modify();
	Atmosphere->Data = MoveTemp(Data);
	Atmosphere->ZoneMap = MoveTemp(Zones);
	Atmosphere->BiomeMap = MoveTemp(Biomes);
	Atmosphere->MapSize = Size;
	return true;
}
