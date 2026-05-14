# ADR 0003: Unix-First Runtime Platform

Ultty is Unix-first for the next phase: Linux and macOS are supported runtime targets, while Windows is future platform work. The current runtime depends on Unix terminal and local transport primitives, so the public contract should stay honest until a Windows named-pipe and ConPTY boundary exists.
