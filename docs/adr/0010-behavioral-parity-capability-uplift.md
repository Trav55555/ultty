# ADR 0010: Behavioral Parity, Capability Uplift

Ulttui dual-mode apps follow behavioral parity with capability uplift. Core workflows should behave consistently in ANSI mode and Ultty-native mode, while native mode may add richer rendering, stronger observability, lower churn, and structured semantics as long as native-only features are explicit and degrade cleanly.
