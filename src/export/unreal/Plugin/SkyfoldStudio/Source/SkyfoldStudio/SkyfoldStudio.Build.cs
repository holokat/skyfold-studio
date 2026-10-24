// Skyfold Studio runtime: the world's weather and atmosphere, the explorer, portals and fires.

using UnrealBuildTool;

public class SkyfoldStudio : ModuleRules
{
	public SkyfoldStudio(ReadOnlyTargetRules Target) : base(Target)
	{
		PCHUsage = PCHUsageMode.UseExplicitOrSharedPCHs;
		PublicDependencyModuleNames.AddRange(new string[] { "Core", "CoreUObject", "Engine", "InputCore" });
		PrivateDependencyModuleNames.AddRange(new string[] { "Json" });
	}
}
