#!/bin/bash
D=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/T22
C=/home/user/supdb/scripts/check.sh
R=$D/mutants3.log
$D/mutants3.sh $R \
 after-lib-new $C 1 $'        RUST_BACKTRACE=0 \\\n        cargo test --profile checked --target aarch64-unknown-linux-gnu --lib 2>&1)' $'        cargo test --profile checked --target aarch64-unknown-linux-gnu --lib RUST_BACKTRACE=0 2>&1)' \
 empty $C 16 'RUST_BACKTRACE=0 \' 'RUST_BACKTRACE= \' \
 short $C 16 'RUST_BACKTRACE=0 \' 'RUST_BACKTRACE=short \' \
 moved-to-test $C 16 $'        RUST_BACKTRACE=0 \\\n\x01      cargo test --profile checked\n      ;;' $'\x01      RUST_BACKTRACE=0 cargo test --profile checked\n      ;;' \
 force-capture /home/user/supdb/src/crypt.rs 16 $'    /// A cipher that panics inside a chunk leaves the chunk as it was\n' $'    #[test]\n    fn a_backtrace_forced() {\n        let b = std::backtrace::Backtrace::force_capture();\n        assert!(!b.to_string().is_empty());\n    }\n\n    /// A cipher that panics inside a chunk leaves the chunk as it was\n'
