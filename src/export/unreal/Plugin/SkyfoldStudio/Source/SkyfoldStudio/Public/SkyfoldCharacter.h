// The explorer, as Skyfold Studio's walk mode: W A S D (or the arrows) to move, the mouse to
// look, Shift to sprint, Alt (or Caps Lock) to walk, C / Ctrl to crouch, Space to jump (and to
// rise while swimming or flying), F to fly or land, R to ride or get down, the mouse wheel for the
// flying speed. Swims in the world's water volumes and keeps track of its breath under water.
// With bEnforceZoneRules the zones' play rules hold (no flying or riding where a zone forbids it).

#pragma once

#include "CoreMinimal.h"
#include "GameFramework/Character.h"
#include "GameFramework/GameModeBase.h"
#include "GameFramework/HUD.h"
#include "SkyfoldCharacter.generated.h"

class UCameraComponent;

UENUM(BlueprintType)
enum class ESkyfoldMove : uint8
{
	Foot,
	Mount,
	Fly,
};

UCLASS(Blueprintable, meta = (DisplayName = "Skyfold Explorer"))
class SKYFOLDSTUDIO_API ASkyfoldCharacter : public ACharacter
{
	GENERATED_BODY()

public:
	ASkyfoldCharacter();

	UPROPERTY(VisibleAnywhere, BlueprintReadOnly, Category = "Skyfold Studio")
	TObjectPtr<UCameraComponent> Camera;

	/** The editor's speeds (m/s, `motion::movement`). */
	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Skyfold Studio|Speeds") float Walk = 2.0f;
	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Skyfold Studio|Speeds") float Run = 6.0f;
	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Skyfold Studio|Speeds") float Sprint = 9.5f;
	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Skyfold Studio|Speeds") float Crouched = 1.8f;
	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Skyfold Studio|Speeds") float Swim = 2.2f;
	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Skyfold Studio|Speeds") float SwimFast = 3.3f;
	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Skyfold Studio|Speeds") float Trot = 4.5f;
	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Skyfold Studio|Speeds") float Gallop = 16.0f;
	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Skyfold Studio|Speeds") float GallopSprint = 22.0f;
	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Skyfold Studio|Speeds") float FlySpeed = 25.0f;
	/** Eye heights (m): standing, crouched, riding. */
	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Skyfold Studio|Speeds") float EyeStand = 1.72f;
	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Skyfold Studio|Speeds") float EyeCrouch = 1.05f;
	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Skyfold Studio|Speeds") float EyeMount = 2.6f;
	/** Seconds of breath under water. */
	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Skyfold Studio") float BreathSeconds = 45.f;
	/** Keep the zones' play rules (Skyfold Studio's walk mode shows them but doesn't). */
	UPROPERTY(EditAnywhere, BlueprintReadWrite, Category = "Skyfold Studio") bool bEnforceZoneRules = false;

	UPROPERTY(BlueprintReadOnly, Category = "Skyfold Studio") ESkyfoldMove Mode = ESkyfoldMove::Foot;
	/** 1 = full lungs, 0 = out of breath. */
	UPROPERTY(BlueprintReadOnly, Category = "Skyfold Studio") float Breath = 1.f;
	UPROPERTY(BlueprintReadOnly, Category = "Skyfold Studio") bool bUnderWater = false;

	UFUNCTION(BlueprintCallable, Category = "Skyfold Studio")
	void SetMode(ESkyfoldMove NewMode);

	virtual void BeginPlay() override;
	virtual void Tick(float DeltaSeconds) override;

private:
	bool bWalkToggle = false;
	float Eye = 1.72f;
};

UCLASS(meta = (DisplayName = "Skyfold Game Mode"))
class SKYFOLDSTUDIO_API ASkyfoldGameMode : public AGameModeBase
{
	GENERATED_BODY()

public:
	ASkyfoldGameMode();
	virtual void InitGame(const FString& MapName, const FString& Options, FString& ErrorMessage) override;
	virtual AActor* ChoosePlayerStart_Implementation(AController* Player) override;
	virtual void RestartPlayer(AController* NewPlayer) override;

private:
	/** "x,y,z,yaw" (cm, degrees) from a portal (`?wbarrive=`): where to arrive. */
	FString Arrive;
};

/** The zone entry banner, the explorer's mode and breath, and the keys. */
UCLASS()
class SKYFOLDSTUDIO_API ASkyfoldHUD : public AHUD
{
	GENERATED_BODY()

public:
	virtual void DrawHUD() override;

	/** H hides it all. */
	bool bHidden = false;
};
