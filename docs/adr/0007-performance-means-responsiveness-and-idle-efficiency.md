# ADR 0007: Performance Means Responsiveness and Idle Efficiency

Ultty defines performance first as idle efficiency and user-visible interaction responsiveness, with microbenchmarks as supporting evidence. Layout and render benchmarks are useful, but they do not substitute for an idle terminal that avoids unnecessary CPU/GPU work or for prompt feedback to input, PTY output, scrolling, search, copy, and native app updates.
