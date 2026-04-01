# recalld
Retrace your steps in Wayland 🐧


## Usage 

### CLI

```console
$ ./target/release/recalld --help
Linux screen recall daemon

Usage: recalld <COMMAND>

Commands:
	daemon  Start the daemon (capture loop + gRPC server)
	search  Search stored screenshots by semantic query
	status  Show daemon status
	ocr-benchmark  Score OCR variants against a local benchmark manifest
	plugin  Manage plugins
	config  Write config file to stdout
	help    Print this message or the help of the given subcommand(s)

Options:
	-h, --help     Print help
	-V, --version  Print version
```

### HTTP Server

The web UI is not a separate service. It is an HTTP server started from the same daemon process that already owns capture, storage, and gRPC.

- The gRPC server and HTTP server are spawned side-by-side from the daemon runtime.
- By default gRPC listens on `[::1]:50051` and HTTP listens on `127.0.0.1:58080`.
- The browser sends the encryption passphrase to the daemon over localhost HTTP for login verification. The daemon uses it to test-unlock key.enc, issues a session cookie on success, and keeps the DEK server-side.
- Screenshot bytes are still stored encrypted on disk and are decrypted by the daemon on demand before being returned over HTTP.
- Search, gallery paging, detail lookup, and screenshot retrieval all execute against the same in-process storage/query layer used by the native API surface.

In practice this means the web path is just another daemon interface, not a second application stack. 

#### UI

> This is a preview, expect functionality to lag behind core features

![](docs/webui-login-preview.png)

![](docs/webui-preview.png)

### Status 

```console
jesse@archl:~/gh/recalld^main ♥
$ RUST_LOG=recalld::daemon=debug,recalld::embedding=debug ./target/release/recalld status
Status:          running
Uptime:          5985s
Total entries:   237
Last capture:    ts=1774961692
Capture backend: auto
Active plugins:  0
```

## Files

```console
/home/jesse/.local/share/recalld
├── key.enc
├── recalld.db
├── recalld.db-shm
├── recalld.db-wal
└── recalld.pid
```


> [!WARNING] 
> 🤖 You didn't write this below...might be correct, totally false or somewhere inbetween 🤷

## Capture Tuning

`capture.similarity_threshold` controls how aggressively recalld skips visually similar screenshots.

- `1.0` is strict: only nearly identical frames are skipped.
- `0.9` allows up to 6 differing perceptual-hash bits out of 64 before storing a new frame.
- `0.1` allows up to 57 differing bits out of 64, which will treat most browser-page changes as unchanged.

The daemon compares each frame against the last captured frame on that monitor, not only the last stored frame.

## Search Ranking

Search now uses a hybrid rank instead of semantic-only cosine.

- Semantic score: embedding cosine similarity (normalized to 0..1).
- Lexical score: normalized token matching plus fuzzy token similarity over decrypted OCR text.
- Final rank: weighted blend controlled by `processing.lexical_weight`.

Use `processing.lexical_weight = 0.0` for semantic-only behavior, `1.0` for lexical-only behavior, and the default `0.35` for a balanced blend that improves literal lookups (URLs, repo names, package names, commit-ish tokens) while preserving conceptual search.

## Runtime Notes

recalld uses Tokio's default multi-thread runtime. OCR, embedding, and storage work run off the async request path, but they are still CPU- and I/O-heavy; high values such as `processing.embedding_threads = 4` can make the desktop feel busy even when the runtime itself is not starved.

recalld intentionally limits thread counts, but it no longer pins heavy work to a fixed subset of CPUs by default. Pinning looked good on paper and turned out to be a bad fit for desktop responsiveness because it could force compositor and input work to compete with OCR and inference on the same cores.

## OCR Benchmarking

Use `recalld ocr-benchmark` to score OCR variants against a private local corpus before changing retrieval.

Example manifest:

```toml
[[cases]]
name = "github-recalld-repo"
url = "https://github.com/jesseconis/recalld"
image = "./benchmarks/screens/github-recalld-repo.png"
expected_text = """
jesseconis/recalld
Retrace your steps in Wayland
README.md
"""
terms = ["jesseconis/recalld", "Retrace your steps in Wayland", "README.md"]
```

If you add a `url` field to a case, you can generate the screenshot automatically with Playwright and bundled Chromium.

Example usage:

```console
$ cp ocr-benchmark.example.toml ocr-benchmark.toml
$ just benchmark-screens manifest=ocr-benchmark.toml
$ recalld ocr-benchmark --manifest ./ocr-benchmark.toml
$ recalld ocr-benchmark --manifest ./ocr-benchmark.toml --variant default --variant no-downscale --variant max-width=1600 --json
```

The daemon still defaults to `processing.ocr_max_width = 1280`. Set `processing.ocr_max_width = 0` to disable downscaling for live captures while testing a full-resolution OCR path.

The screenshot generator captures the primary page content for each public case URL and writes PNGs under `benchmarks/screens/`. Cases without `url` are skipped.
