// Skyfold Studio import helpers for the editor (called from import_world.py).

using UnrealBuildTool;

public class SkyfoldStudioEditor : ModuleRules
{
	public SkyfoldStudioEditor(ReadOnlyTargetRules Target) : base(Target)
	{
		PCHUsage = PCHUsageMode.UseExplicitOrSharedPCHs;
		PublicDependencyModuleNames.AddRange(new string[] { "Core", "CoreUObject", "Engine", "Landscape", "SkyfoldStudio" });
		PrivateDependencyModuleNames.AddRange(new string[] { "UnrealEd", "AssetRegistry" });
	}
}
