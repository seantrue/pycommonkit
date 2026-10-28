# commonkit

Named shared-memory segments for numpy `.npy` arrays: one process publishes an
array, and every other process on the host maps the same physical pages,
read-only.

This is the Rust side of [pycommonkit](https://github.com/seantrue/pycommonkit).
Both implement one on-disk contract,
[`docs/contract.md`](https://github.com/seantrue/pycommonkit/blob/main/docs/contract.md),
and a cross-language conformance suite runs both on one tree. A Rust process can
publish, attach and reap as fully as a Python one, and either can read what the
other wrote. It does not wrap Python, and needs no Python at run time.

## The idea

A segment is a `.npy` file under a store root (by default `/dev/shm/commonkit`
on Linux, `~/Library/Caches/commonkit/store` on macOS), named by a key such as `demo:v1:f32:{md5}:x0`. Any process that can
spell the key reaches the same bytes, without knowing who wrote them. The name
comes from Fortran's named `COMMON` block.

**A missing segment is never an error, only a slower path.** Nothing on the
read, publish or reap path panics or returns an error for an absent root, a full
disk, a reaped or torn segment or an unexpected dtype; those are counted misses,
and the caller recomputes. The two loud paths are invalid key or scheme input,
and publishing a different array under an existing key (`KeyCollision`).

A reaper, run in turns after publishes and declines, evicts expired and least
recently used segments toward a byte budget and a free-space floor. It only ever
unlinks regular files of the segment shape under the root.

## Example

```rust,no_run
use commonkit::{ArrayRef, AttachMode, Descr, KeyScheme, Mode, Order, Variants, keys};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A scheme is one domain's vocabulary: forms, variants and, optionally, dtypes.
    let scheme = keys::register(KeyScheme::new(
        "demo", "v1", ["f32"], Variants::closed(["x0"])?, &[("f32", "<f4")])?)?;
    let key = scheme.key("0123456789abcdef0123456789abcdef", "x0", None)?;

    let store = commonkit::shared_store(Mode::ReadWrite, AttachMode::Mmap, 0);
    let descr = Descr::parse("<f4")?;
    let data = [0u8; 16];
    store.publish(key.as_str(), ArrayRef::new(&descr, &[4], Order::C, &data)?)?;

    // In this or any other process: a miss is `None`, and the caller recomputes.
    if let Some(seg) = store.attach(key.as_str()) {
        let values: Option<&[f32]> = seg.as_slice();
        println!("{values:?}");
    }
    Ok(())
}
```

`shared_store` is the one per-host store every user shares; its policy (root,
byte budget, TTL, free-space floor) comes from `{root}/.policy.json`, then
`COMMONKIT_STORE_*` environment variables, then platform defaults.

## Features

- `mmap` (default): attach maps segments read-only through `memmap2`. This is
  the crate's only `unsafe`, in one module. Without it no `unsafe` is compiled,
  and attach copies instead.

## Platforms

Linux and macOS are tested. Windows builds and is supported, but is not yet run
in CI.

Minimum supported Rust version: 1.95.

## License

MIT.
