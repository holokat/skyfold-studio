#include "SkyfoldPortal.h"

#include "Components/BoxComponent.h"
#include "Components/StaticMeshComponent.h"
#include "Engine/World.h"
#include "Components/PointLightComponent.h"
#include "GameFramework/Pawn.h"
#include "Kismet/GameplayStatics.h"

ASkyfoldPortal::ASkyfoldPortal()
{
	PrimaryActorTick.bCanEverTick = true;
	Box = CreateDefaultSubobject<UBoxComponent>(TEXT("Box"));
	Box->SetBoxExtent(FVector(100.f, 100.f, 150.f));
	Box->SetCollisionProfileName(TEXT("Trigger"));
	RootComponent = Box;
}

void ASkyfoldPortal::BeginPlay()
{
	Super::BeginPlay();
	Box->OnComponentBeginOverlap.AddDynamic(this, &ASkyfoldPortal::OnOverlap);
}

void ASkyfoldPortal::OnOverlap(UPrimitiveComponent*, AActor* Other, UPrimitiveComponent*, int32, bool, const FHitResult&)
{
	// (not while the arrival is still standing in it)
	if (GetWorld()->GetTimeSeconds() < Armed)
	{
		return;
	}
	const APawn* P = Cast<APawn>(Other);
	if (P && P->IsPlayerControlled())
	{
		Travel();
	}
}

void ASkyfoldPortal::Travel()
{
	if (bGone || Level.IsNone())
	{
		return;
	}
	bGone = true;
	const FString Options = FString::Printf(TEXT("wbarrive=%.1f,%.1f,%.1f,%.1f"), ArriveLocation.X, ArriveLocation.Y, ArriveLocation.Z, ArriveYaw);
	UGameplayStatics::OpenLevel(this, Level, true, Options);
}

ASkyfoldFire::ASkyfoldFire()
{
	PrimaryActorTick.bCanEverTick = true;
	RootComponent = CreateDefaultSubobject<USceneComponent>(TEXT("Root"));
	Flame = CreateDefaultSubobject<UStaticMeshComponent>(TEXT("Flame"));
	Flame->SetupAttachment(RootComponent);
	Flame->SetCollisionEnabled(ECollisionEnabled::NoCollision);
	Flame->SetCastShadow(false);
	Flame->SetUsingAbsoluteRotation(true);
	Light = CreateDefaultSubobject<UPointLightComponent>(TEXT("Light"));
	Light->SetupAttachment(RootComponent);
	Light->SetCastShadows(false);
}

void ASkyfoldFire::BeginPlay()
{
	Super::BeginPlay();
	BaseIntensity = Light->Intensity;
	Phase = FMath::FRandRange(0.f, 100.f);
}

void ASkyfoldFire::OnConstruction(const FTransform& Transform)
{
	Super::OnConstruction(Transform);
	// what the flame material reads: its phase and pace, spread and size; bounds round the
	// upright sheet it draws (the mesh itself is flat)
	Flame->SetCustomPrimitiveDataFloat(0, Seed);
	Flame->SetCustomPrimitiveDataFloat(1, Spread);
	Flame->SetCustomPrimitiveDataFloat(2, Size.X);
	Flame->SetCustomPrimitiveDataFloat(3, Size.Y);
	Flame->SetBoundsScale(FMath::Max(Size.X, Size.Y) / 50.f + 1.f);
}

void ASkyfoldFire::Tick(float DeltaSeconds)
{
	Super::Tick(DeltaSeconds);
	Phase += DeltaSeconds;
	// two drifting sines and a little noise, like the editor's fire light
	const float N = 0.55f * FMath::Sin(Phase * 7.3f) + 0.3f * FMath::Sin(Phase * 13.1f + 1.7f) + 0.15f * FMath::FRandRange(-1.f, 1.f);
	Light->SetIntensity(BaseIntensity * (1.f + Flicker * N));
}
