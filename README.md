# Rust Compiler Fuzzer

An experimental attempt to build an AI-assisted fuzzer for finding miscompilations in the Rust compiler.

The project uses multiple GitHub Actions workflows to generate and test Rust programs in an effort to identify compiler bugs.

Tests are performed by generating large Rust programs composed of multiple smaller code fragments. Each program is compiled with different compiler flags, and the resulting binaries are executed and their outputs compared to detect inconsistencies that may indicate miscompilations.

The open Rust miscompilation issues serve as a reference for understanding what real-world miscompilations look like and how to generate test cases that may trigger similar bugs(https://github.com/rust-lang/rust/issues?q=state%3Aopen%20label%3A%22I-miscompile%22)

Tests are run against the latest Rust nightly toolchain to check whether potential issues still exist or have already been fixed.

## TODO

Implement multiple strategies for generating Rust programs. Different compiler bugs may require different approaches, such as generating complex lifetime relationships, trait interactions, generic code, or other language constructs that are difficult for the compiler to handle correctly.
