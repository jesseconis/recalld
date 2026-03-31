# recalld
Retrace your steps in Wayland 🐧



> [!WARNING] 
> 🤖 You didn't write this...might be correct, totally false or somewhere inbetween 🤷

## Capture Tuning

`capture.similarity_threshold` controls how aggressively recalld skips visually similar screenshots.

- `1.0` is strict: only nearly identical frames are skipped.
- `0.9` allows up to 6 differing perceptual-hash bits out of 64 before storing a new frame.
- `0.1` allows up to 57 differing bits out of 64, which will treat most browser-page changes as unchanged.

The daemon compares each frame against the last captured frame on that monitor, not only the last stored frame.

## Runtime Notes

recalld uses Tokio's default multi-thread runtime. OCR, embedding, and storage work run off the async request path, but they are still CPU- and I/O-heavy; high values such as `processing.embedding_threads = 4` can make the desktop feel busy even when the runtime itself is not starved.

recalld intentionally limits thread counts, but it no longer pins heavy work to a fixed subset of CPUs by default. Pinning looked good on paper and turned out to be a bad fit for desktop responsiveness because it could force compositor and input work to compete with OCR and inference on the same cores.
