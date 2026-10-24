#include "SkyfoldCharacter.h"

#include "Camera/CameraComponent.h"
#include "Components/CapsuleComponent.h"
#include "Engine/Canvas.h"
#include "Engine/Font.h"
#include "Engine/GameViewportClient.h"
#include "Engine/LocalPlayer.h"
#include "GameFramework/CharacterMovementComponent.h"
#include "GameFramework/PhysicsVolume.h"
#include "GameFramework/PlayerController.h"
#include "InputCoreTypes.h"
#include "Kismet/GameplayStatics.h"
#include "SkyfoldAtmosphere.h"

namespace
{
	constexpr float HalfHeight = 90.f;
	/** The editor's gravity (m/s²) over Unreal's. */
	constexpr float GravityScale = 22.f / 9.81f;
}

ASkyfoldCharacter::ASkyfoldCharacter()
{
	PrimaryActorTick.bCanEverTick = true;
	GetCapsuleComponent()->InitCapsuleSize(35.f, HalfHeight);
	Camera = CreateDefaultSubobject<UCameraComponent>(TEXT("Camera"));
	Camera->SetupAttachment(GetCapsuleComponent());
	Camera->SetRelativeLocation(FVector(0.f, 0.f, EyeStand * 100.f - HalfHeight));
	Camera->bUsePawnControlRotation = true;
	Camera->SetFieldOfView(70.f);
	bUseControllerRotationYaw = true;
	// arrive even where the start or a portal's arrival overlaps a prop (nudged clear)
	SpawnCollisionHandlingMethod = ESpawnActorCollisionHandlingMethod::AdjustIfPossibleButAlwaysSpawn;
	UCharacterMovementComponent* M = GetCharacterMovement();
	M->GravityScale = GravityScale;
	M->JumpZVelocity = 700.f;
	M->AirControl = 0.35f;
	M->MaxWalkSpeed = Run * 100.f;
	M->MaxWalkSpeedCrouched = Crouched * 100.f;
	M->MaxSwimSpeed = Swim * 100.f;
	M->MaxFlySpeed = FlySpeed * 100.f;
	M->BrakingDecelerationFlying = 4000.f;
	M->Buoyancy = 1.05f;
	M->GetNavAgentPropertiesRef().bCanCrouch = true;
	M->GetNavAgentPropertiesRef().bCanSwim = true;
	M->GetNavAgentPropertiesRef().bCanFly = true;
	M->bCanWalkOffLedgesWhenCrouching = true;
	M->SetWalkableFloorAngle(50.f);
	M->MaxStepHeight = 45.f;
}

void ASkyfoldCharacter::BeginPlay()
{
	Super::BeginPlay();
	if (APlayerController* PC = Cast<APlayerController>(GetController()))
	{
		PC->SetInputMode(FInputModeGameOnly());
		PC->bShowMouseCursor = false;
	}
}

void ASkyfoldCharacter::SetMode(ESkyfoldMove NewMode)
{
	ASkyfoldAtmosphere* Atm = ASkyfoldAtmosphere::Get(this);
	if (bEnforceZoneRules && Atm)
	{
		if ((NewMode == ESkyfoldMove::Fly && !Atm->bAllowFlying) || (NewMode == ESkyfoldMove::Mount && !Atm->bAllowMounts))
		{
			return;
		}
	}
	UCharacterMovementComponent* M = GetCharacterMovement();
	if (NewMode == ESkyfoldMove::Mount && M->IsInWater())
	{
		return; // (no horse in deep water)
	}
	Mode = NewMode;
	M->SetMovementMode(Mode == ESkyfoldMove::Fly ? MOVE_Flying : MOVE_Walking);
}

void ASkyfoldCharacter::Tick(float DeltaSeconds)
{
	Super::Tick(DeltaSeconds);
	APlayerController* PC = Cast<APlayerController>(GetController());
	if (PC == nullptr)
	{
		return;
	}
	UCharacterMovementComponent* M = GetCharacterMovement();
	auto Down = [PC](const FKey& K) { return PC->IsInputKeyDown(K); };
	auto Pressed = [PC](const FKey& K) { return PC->WasInputKeyJustPressed(K); };
	// look (only while the game's viewport has the focus: an unfocused window reports stray deltas)
	const ULocalPlayer* LP = PC->GetLocalPlayer();
	const bool bFocused = LP && LP->ViewportClient && LP->ViewportClient->Viewport && LP->ViewportClient->Viewport->HasFocus();
	if (bFocused)
	{
		float DX = 0.f, DY = 0.f;
		PC->GetInputMouseDelta(DX, DY);
		AddControllerYawInput(DX * 0.5f);
		AddControllerPitchInput(-DY * 0.5f);
	}
	// modes
	if (Pressed(EKeys::F))
	{
		SetMode(Mode == ESkyfoldMove::Fly ? ESkyfoldMove::Foot : ESkyfoldMove::Fly);
	}
	if (Pressed(EKeys::R))
	{
		SetMode(Mode == ESkyfoldMove::Mount ? ESkyfoldMove::Foot : ESkyfoldMove::Mount);
	}
	if (Pressed(EKeys::CapsLock))
	{
		bWalkToggle = !bWalkToggle;
	}
	if (Pressed(EKeys::H))
	{
		if (ASkyfoldHUD* Hud = Cast<ASkyfoldHUD>(PC->GetHUD()))
		{
			Hud->bHidden = !Hud->bHidden;
		}
	}
	if (Mode == ESkyfoldMove::Fly)
	{
		if (Pressed(EKeys::MouseScrollUp)) { FlySpeed = FMath::Min(FlySpeed * 1.25f, 300.f); }
		if (Pressed(EKeys::MouseScrollDown)) { FlySpeed = FMath::Max(FlySpeed / 1.25f, 2.f); }
	}
	const bool bSprint = Down(EKeys::LeftShift) || Down(EKeys::RightShift);
	const bool bWalk = Down(EKeys::LeftAlt) || Down(EKeys::RightAlt) || bWalkToggle;
	const bool bCrouch = Down(EKeys::C) || Down(EKeys::LeftControl) || Down(EKeys::RightControl);
	const float Fwd = float(Down(EKeys::W) || Down(EKeys::Up)) - float(Down(EKeys::S) || Down(EKeys::Down));
	const float Right = float(Down(EKeys::D) || Down(EKeys::Right)) - float(Down(EKeys::A) || Down(EKeys::Left));
	// speeds by mode and gait
	const bool bSwimming = M->IsSwimming();
	if (Mode == ESkyfoldMove::Fly)
	{
		M->MaxFlySpeed = FlySpeed * 100.f * (bSprint ? 2.f : 1.f);
	}
	else if (Mode == ESkyfoldMove::Mount)
	{
		M->MaxWalkSpeed = (bWalk ? Trot : bSprint ? GallopSprint : Gallop) * 100.f;
	}
	else
	{
		M->MaxWalkSpeed = (bWalk ? Walk : bSprint ? Sprint : Run) * 100.f;
	}
	M->MaxSwimSpeed = (bSprint ? SwimFast : Swim) * 100.f;
	if (Mode == ESkyfoldMove::Foot && !bSwimming)
	{
		if (bCrouch && !bIsCrouched) { Crouch(); }
		if (!bCrouch && bIsCrouched) { UnCrouch(); }
	}
	// move: on the ground along the view's heading, flying and swimming along the view
	const FRotator View = GetControlRotation();
	const bool bFree = Mode == ESkyfoldMove::Fly || bSwimming;
	const FRotator Basis = bFree ? View : FRotator(0.f, View.Yaw, 0.f);
	AddMovementInput(FRotationMatrix(Basis).GetUnitAxis(EAxis::X), Fwd);
	AddMovementInput(FRotationMatrix(Basis).GetUnitAxis(EAxis::Y), Right);
	if (bFree)
	{
		const float Up = float(Down(EKeys::SpaceBar)) - float(bCrouch);
		AddMovementInput(FVector::UpVector, Up);
	}
	else if (Pressed(EKeys::SpaceBar))
	{
		Jump();
	}
	// the eye: standing, crouched, riding (eased)
	const float WantEye = Mode == ESkyfoldMove::Mount ? EyeMount : (bIsCrouched ? EyeCrouch : EyeStand);
	Eye = FMath::FInterpTo(Eye, WantEye, DeltaSeconds, 8.f);
	const float Base = bIsCrouched ? GetCharacterMovement()->GetCrouchedHalfHeight() : HalfHeight;
	Camera->SetRelativeLocation(FVector(0.f, 0.f, Eye * 100.f - Base));
	// breath: the eye under the water's surface
	bUnderWater = false;
	if (APhysicsVolume* Vol = GetPhysicsVolume())
	{
		if (Vol->bWaterVolume)
		{
			const FBox B = Vol->GetComponentsBoundingBox(true);
			bUnderWater = Camera->GetComponentLocation().Z < B.Max.Z;
		}
	}
	Breath = FMath::Clamp(Breath + (bUnderWater ? -DeltaSeconds / BreathSeconds : DeltaSeconds * 3.f / BreathSeconds), 0.f, 1.f);
}

ASkyfoldGameMode::ASkyfoldGameMode()
{
	DefaultPawnClass = ASkyfoldCharacter::StaticClass();
	HUDClass = ASkyfoldHUD::StaticClass();
}

void ASkyfoldGameMode::InitGame(const FString& MapName, const FString& Options, FString& ErrorMessage)
{
	Super::InitGame(MapName, Options, ErrorMessage);
	Arrive = UGameplayStatics::ParseOption(Options, TEXT("wbarrive"));
}

AActor* ASkyfoldGameMode::ChoosePlayerStart_Implementation(AController* Player)
{
	return Super::ChoosePlayerStart_Implementation(Player);
}

void ASkyfoldGameMode::RestartPlayer(AController* NewPlayer)
{
	Super::RestartPlayer(NewPlayer);
	if (NewPlayer && NewPlayer->GetPawn() == nullptr)
	{
		UE_LOG(LogTemp, Warning, TEXT("Skyfold Studio: the explorer couldn't be placed at the player start"));
	}
	TArray<FString> P;
	Arrive.ParseIntoArray(P, TEXT(","));
	if (P.Num() >= 3 && NewPlayer && NewPlayer->GetPawn())
	{
		const FVector At(FCString::Atof(*P[0]), FCString::Atof(*P[1]), FCString::Atof(*P[2]));
		const float Yaw = P.Num() >= 4 ? FCString::Atof(*P[3]) : 0.f;
		NewPlayer->GetPawn()->TeleportTo(At, FRotator(0.f, Yaw, 0.f));
		NewPlayer->SetControlRotation(FRotator(0.f, Yaw, 0.f));
	}
}

void ASkyfoldHUD::DrawHUD()
{
	Super::DrawHUD();
	if (bHidden || Canvas == nullptr)
	{
		return;
	}
	UFont* Big = GEngine ? GEngine->GetLargeFont() : nullptr;
	UFont* Small = GEngine ? GEngine->GetSmallFont() : nullptr;
	const float W = Canvas->ClipX, H = Canvas->ClipY;
	auto Centered = [&](const FString& Text, UFont* Font, float Y, float Scale, const FLinearColor& Col) {
		float TW = 0.f, TH = 0.f;
		GetTextSize(Text, TW, TH, Font, Scale);
		DrawText(Text, Col, (W - TW) * 0.5f, Y, Font, Scale);
	};
	// the zone entry banner (fades out after a few seconds, as the editor's)
	if (ASkyfoldAtmosphere* Atm = ASkyfoldAtmosphere::Get(this))
	{
		if (!Atm->ZoneName.IsEmpty() && Atm->ZoneAge < 4.5f)
		{
			const float A = FMath::Clamp(FMath::Min(Atm->ZoneAge / 0.4f, (4.5f - Atm->ZoneAge) / 1.f), 0.f, 1.f);
			Centered(Atm->ZoneName, Big, H * 0.16f, 2.f, FLinearColor(1.f, 0.96f, 0.86f, A));
			Centered(Atm->ZoneBanner, Small, H * 0.16f + 56.f, 1.3f, FLinearColor(0.9f, 0.9f, 0.9f, A));
		}
	}
	if (ASkyfoldCharacter* Me = Cast<ASkyfoldCharacter>(GetOwningPawn()))
	{
		if (Me->bUnderWater || Me->Breath < 0.999f)
		{
			const float BW = 220.f, BX = (W - BW) * 0.5f, BY = H - 60.f;
			DrawRect(FLinearColor(0.f, 0.f, 0.f, 0.4f), BX - 2.f, BY - 2.f, BW + 4.f, 12.f);
			DrawRect(FLinearColor(0.55f, 0.8f, 1.f, 0.9f), BX, BY, BW * Me->Breath, 8.f);
		}
		const TCHAR* ModeName = Me->Mode == ESkyfoldMove::Fly ? TEXT("Flying") : Me->Mode == ESkyfoldMove::Mount ? TEXT("Riding") : TEXT("On foot");
		DrawText(FString::Printf(TEXT("%s   ·   WASD move · Shift sprint · Alt walk · C crouch · Space jump · F fly · R ride · H hide"), ModeName),
			FLinearColor(1.f, 1.f, 1.f, 0.75f), 16.f, H - 28.f, Small, 1.f);
	}
}
