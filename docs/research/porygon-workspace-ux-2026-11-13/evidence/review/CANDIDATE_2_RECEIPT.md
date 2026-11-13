# Candidate 2 runtime receipt

Client `4581ed9`, binary SHA256 `356149eaa7e1ceb0397af45b3ffc783394b48c4747690f1ed513c247834809bb`, ran against immutable daemon `abc12c1364a61c4c045ffdaee230a7aed08159f8` and the frozen current Ultty/ulttyctl binaries.

ANSI80 and ANSI160 each passed six gates in39.81s and40.27s. Native passed the full 10,000-message/work/attention/clipboard scenario in17.28s. Native result directory is `receipts/native-1794592391164772241`. Pixel inspection confirmed single JSON braces/brackets, readable raw artifact paths, oldest00000 and latest09999, and editing at the latest boundary.

The history opening times were2.766s/2.790s/2.853s for ANSI80/ANSI160/native. Empirical p95 over20 driver-inclusive edit observations was123ms/132ms/115ms. Child VmRSS was32,720/34,072/32,740kB. These do not establish frame latency, human usability or provider performance.

This was diagnostic evidence for authority because the old daemon contract had no version-bound request. Final approval acceptance is candidate3 against b9d524dc, recorded separately. No earlier failure or diagnostic was overwritten.
