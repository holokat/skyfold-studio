// A way between the world and a dungeon (or back): walking into its box opens the level, the
// explorer arriving where the portal says (`ASkyfoldGameMode` reads `?wbarrive=`).

#pragma once

#include "CoreMinimal.h"
#include "GameFramework/Actor.h"
#include "SkyfoldPortal.generated.h"

class UBoxComponent;

UCLASS(Blueprintable, meta = (DisplayName = "Skyfold Portal"))
class SKYFOLDSTUDIO_API ASkyfoldPortal : public AActor
{
	GENERATED_BODY()

public:
	ASkyfoldPortal();

	UPROPERTY(VisibleAnywhere, BlueprintReadOnly, Category = "Skyfold Studio")
	TObjectPtr<UBoxComponent> Box;
	/** The level to open (its name or package path). */
	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Skyfold Studio")
	FName Level;
	/** Where to arrive in it: x, y, z (cm) and yaw (degrees). */
	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Skyfold Studio")
	FVector ArriveLocation = FVector::ZeroVector;
	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Skyfold Studio")
	float ArriveYaw = 0.f;
	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Skyfold Studio")
	FString Label;

	UFUNCTION(BlueprintCallable, Category = "Skyfold Studio")
	void Travel();

protected:
	virtual void BeginPlay() override;

private:
	UFUNCTION()
	void OnOverlap(UPrimitiveComponent* Comp, AActor* Other, UPrimitiveComponent* OtherComp, int32 Body, bool bSweep, const FHitResult& Hit);
	float Armed = 1.f;
	bool bGone = false;
};

/** A fire from Skyfold Studio's flame flipbooks: an animated sheet (its material stands the
 * quad upright towards the view, in game and in the editor) and its flickering light. */
UCLASS(Blueprintable, meta = (DisplayName = "Skyfold Fire"))
class SKYFOLDSTUDIO_API ASkyfoldFire : public AActor
{
	GENERATED_BODY()

public:
	ASkyfoldFire();

	/** The sheet on a 100 cm plane mesh; its material sizes it (Size) and faces it to the view. */
	UPROPERTY(VisibleAnywhere, BlueprintReadOnly, Category = "Skyfold Studio")
	TObjectPtr<class UStaticMeshComponent> Flame;
	/** The flame's width and height (cm). */
	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Skyfold Studio")
	FVector2D Size = FVector2D(50.f, 80.f);
	/** This fire's phase and pace (0..1) and how many sheet widths its bed spans. */
	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Skyfold Studio")
	float Seed = 0.f;
	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Skyfold Studio")
	float Spread = 1.f;
	UPROPERTY(VisibleAnywhere, BlueprintReadOnly, Category = "Skyfold Studio")
	TObjectPtr<class UPointLightComponent> Light;
	/** How much the light flickers (0 steady). */
	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Skyfold Studio")
	float Flicker = 0.25f;

	virtual void Tick(float DeltaSeconds) override;
	virtual void OnConstruction(const FTransform& Transform) override;

protected:
	virtual void BeginPlay() override;

private:
	float BaseIntensity = -1.f;
	float Phase = 0.f;
};
