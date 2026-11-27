/* A stand-in for Apple's header, for ring's build when the lint group type-checks the macOS target on Linux (scripts/check.sh lint); its objects are never linked. */
#pragma once
#define TARGET_OS_MAC 1
#define TARGET_OS_OSX 1
#define TARGET_OS_IPHONE 0
#define TARGET_OS_IOS 0
#define TARGET_OS_TV 0
#define TARGET_OS_WATCH 0
#define TARGET_OS_SIMULATOR 0
#define TARGET_OS_EMBEDDED 0
#define TARGET_CPU_ARM64 1
