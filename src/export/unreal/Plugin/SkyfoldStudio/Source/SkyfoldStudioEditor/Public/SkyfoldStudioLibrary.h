// Skyfold Studio import helpers that Unreal's Python API doesn't offer, for import_world.py
// (unreal.SkyfoldStudioLibrary).

#pragma once

#include "CoreMinimal.h"
#include "Kismet/BlueprintFunctionLibrary.h"
#include "SkyfoldStudioLibrary.generated.h"

class ALandscape;
class ALandscapeProxy;
class ASkyfoldAtmosphere;
class UMaterialInterface;

UCLASS()
class SKYFOLDSTUDIOEDITOR_API USkyfoldStudioLibrary : public UBlueprintFunctionLibrary
{
	GENERATED_BODY()

public:
	/**
	 * Creates a Landscape in the editor world as Landscape mode's "Import from File" does: heights
	 * from a raw 16-bit little-endian file of Resolution x Resolution samples (row-major along
	 * Unreal +Y, columns along +X), one weight-blended target layer per name with its 8-bit raw
	 * weights (same layout; a layer info asset each in LayerInfoFolder), and the cave holes
	 * (8-bit, 255 = hole) as the visibility layer. Resolution - 1 must be a multiple of 126
	 * (2 x 2 sections of 63 quads per component). Location is the landscape's corner (cm).
	 */
	UFUNCTION(BlueprintCallable, Category = "Skyfold Studio", meta = (DevelopmentOnly))
	static ALandscape* CreateLandscape(const FString& HeightFile, int32 Resolution, FVector Location, FVector Scale,
		UMaterialInterface* Material, const TArray<FString>& LayerNames, const TArray<FString>& LayerFiles,
		const FString& LayerInfoFolder, const FString& HoleFile, const FString& Label);

	/** A one-line summary of a landscape for checks: components, and the target layers painted
	 * in them (how many components carry each). */
	UFUNCTION(BlueprintCallable, Category = "Skyfold Studio", meta = (DevelopmentOnly))
	static FString DescribeLandscape(ALandscapeProxy* Landscape);

	/** Fills a SkyfoldAtmosphere with Skyfold Studio's atmosphere.json and its zone and biome
	 * maps (one byte per terrain vertex, Size x Size). */
	UFUNCTION(BlueprintCallable, Category = "Skyfold Studio", meta = (DevelopmentOnly))
	static bool LoadAtmosphere(ASkyfoldAtmosphere* Atmosphere, const FString& JsonFile, const FString& ZonesFile, const FString& BiomesFile, int32 Size);
};
