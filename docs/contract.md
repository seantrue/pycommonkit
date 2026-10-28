---
title: "The commonkit on-disk contract"
subtitle: "Version 1 (draft for the maintainer's acceptance)"
date: 2026-10-27
---

# The commonkit on-disk contract, version 1

## 1. Status, authority and conventions

### 1.1 What this is

This document is the authority on what a commonkit store looks like on disk and how a process that publishes, attaches or reaps in it must behave. Any implementation, in any language, conforms when it follows this document; it can be implemented without reading the Python source.

- **The reference implementation** is the Python package (`src/commonkit`, imported as `commonkit`).
- **The arbiter** is the conformance vector set in `conformance/vectors/` (section 13). Where this text and a vector disagree, the disagreement is a bug in one of them, to be fixed; until it is, the vector wins.
- **The contract versions separately** from the packages (plan decision C11). This is contract version 1.

### 1.2 Requirement words

MUST, MUST NOT, SHOULD and MAY are used as in RFC 2119. Where nothing is said, behaviour is unspecified and no conforming process may depend on it.

### 1.3 Where Python changes

In a few places this contract, as the maintainer decided it, runs ahead of the Python package as it stood at commonkit `2b6bcf9`. Each such clause was marked **"Python: changes in step 3"**, and Appendix A lists them all; the matching vectors carried `"python": "step3"` and were expected to fail in Python. Step 3 of `docs/rust-crate-plan.md` has landed: each clause now reads "Python conforms since pycommonkit 0.1.0 (A*n*)", and no vector carries the mark. A later change that runs ahead of Python again uses the same marks.

### 1.4 Section numbers and the plan

`docs/rust-crate-plan.md` cites the contract as `§6.1` to `§6.9`, its own section 6. Its section *N* is section *N + 2* here:

| Plan | This document | Subject |
|---|---|---|
| "The write-once rules" | §2 | The write-once rules |
| §6.1 | §3 | Keys and schemes |
| §6.2 | §4 | Layout |
| §6.3 | §5 | The `.npy` segment |
| §6.4 | §6 | Publishing |
| §6.5 | §7 | The mutation rule |
| §6.6 | §8 | Reading and touch |
| §6.7 | §9 | The reaper |
| §6.8 | §10 | Policy |
| §6.9 | §11 | Counters |
| (C10) | §12 | Windows |
| (C2, C3) | §13 | Conformance vectors |

So the `SAFETY:` comment the plan asks for (plan §7.2, "cites contract §6.5") cites §7 of this document.

### 1.5 Terms

- **Root:** the directory a store lives under (§10.4).
- **Segment:** a regular file at a segment path (§4) holding one array in `.npy` form (§5).
- **Publisher:** a process that writes segments. **Reader:** one that attaches them. **Reaper:** one that removes them (§9). One process may be all three.
- **Miss:** an attach that returns no array. **Decline:** a publish that writes nothing, by policy. Both are normal operation, not faults.
- **Counter:** a named non-negative integer a store increments (§11).

### 1.6 The never-raise invariant

A missing segment is never an error, only a slower path. On the read, publish and reap paths an implementation MUST NOT raise, panic or return an error to its caller for any condition named in this document except the two **loud paths**:

1. building a key, or constructing or registering a scheme, from invalid input (§3);
2. a key collision on publish (§6.4).

Everything else (absent root, full disk, missing, reaped or torn segment, bad key on read or publish, unacceptable dtype) is a miss or a decline, reported by a return value plus a counter. Constructing a store MUST NOT fail: an unusable root yields a **disabled** store (§6.1).

## 2. The write-once rules

These rules are the contract's foundation; the sections below make them precise.

- **W1. Write once.** The store is write-once by key. A key names one immutable content. A publisher MUST NOT publish different bytes under a key whose segment already exists. Republishing the same array is a touch, never a write (§6.4). A different array under an existing key is a **key collision**, a major system error that fails loudly (§6.4). A change of content is a new key: a new digest or a new scheme `version`.
- **W2. Temp-then-rename.** Every writer writes the whole segment to a private temporary file in the segment's own directory, then renames it over the final path (§6.5). A segment never exists under its final name in part.
- **W3. Unlink, never truncate.** Segments are removed only by unlinking. No conforming process truncates a segment, writes into it in place, or opens it for writing (§7).
- **W4. Short is a miss.** A reader treats a file shorter than its header promises, or whose header does not parse, as a torn miss and counts `shm_torn` (§5.7, §8.1). This is a safety net for faults outside the contract (a crash with fsync off, a non-conforming tool, disk corruption), not part of normal operation.
- **W5. Durability is policy.** fsync is a policy option, off by default (§6.5, §10). With it off, W4 covers a crash that leaves a short file.

## 3. Keys and schemes

### 3.1 Key grammar

A key is an ASCII string of exactly five components separated by `:`:

```text
{domain}:{version}:{form}:{digest}:{variant}
```

- `domain`, `version`, `form` and `variant` are **tokens**: one or more characters from `[a-z0-9]`.
- `digest` is exactly 32 characters from `[0-9a-f]`.
- The whole key MUST match this expression **in full**, with no leading or trailing character of any kind (a final newline included):

  ```text
  [a-z0-9]+:[a-z0-9]+:[a-z0-9]+:[0-9a-f]{32}:[a-z0-9]+
  ```

  Python conforms since pycommonkit 0.1.0 (A1): it matches with `fullmatch`. Its patterns used to end in `$`, which also matches before a final `\n`, so a key or token with a trailing newline was accepted (vectors `keys.parse.trailing_newline`, `keys.build.digest_newline`).

The grammar alone guarantees that a key's path (§3.6) stays under the root: no component can be empty, contain a separator, start with `.`, or be `..`.

### 3.2 Schemes

A **scheme** fixes one domain's vocabulary:

- `domain`: a token.
- `version`: a token. It is the decode contract, not a release number: bump it to orphan every existing segment of the domain.
- `forms`: a non-empty, ordered sequence of distinct tokens. The first is the **default form**. Form names are global across domains in a shared tree, because the form is the first path component (§4).
- `variants`: either a non-empty closed set of tokens, or a **variant pattern**, a regular expression for an open axis such as sample rates. A variant is valid for the scheme when it is a token (§3.1) **and** it is in the closed set, or the pattern matches the **whole** variant.
  - Patterns SHOULD use only the syntax common to Python's `re` and Rust's `regex` crate: literals, character classes, `.`, quantifiers, alternation and groups; no backreferences or look-around.
  - Python conforms since pycommonkit 0.1.0 (A2). It used to match a pattern at the start of the variant only (`re.match`), so `sr[0-9]+` accepted `sr16000x`, and it did not check that a pattern-accepted variant is a token, so `sr.+` built a key with `sr-1` that no parser accepts (vectors `keys.build.pattern_prefix_only`, `keys.build.pattern_breaks_grammar`).
- `dtypes` (optional): a map from some of the scheme's forms to the one dtype a segment of that form must have (§6.3). Every key MUST be one of `forms`. In this document a dtype is named by its canonical descriptor (§5.4), such as `|u1` or `<f4`; an implementation MAY accept other spellings (Python accepts any `np.dtype` spelling, such as `uint8`) provided it compares by dtype identity.

Constructing a scheme from invalid input is a loud path (§1.6): Python raises `ValueError`; other languages return an error.

### 3.3 Building and parsing

- **Building** a key from a scheme takes a digest, a variant and an optional form (default: the default form).
  - The digest is first **folded to lower case** (ASCII letters only), then MUST match `[0-9a-f]{32}` in full.
  - The form MUST be one of the scheme's forms; the variant MUST be valid (§3.2). Variants and forms are never folded.
  - Any failure is a loud path (§1.6).
- **Parsing** a key against a scheme returns `(form, digest, variant)`. It **never folds**: an upper-case digest is unparseable. The key MUST satisfy the grammar (§3.1), and its domain and version MUST equal the scheme's, its form MUST be one of the scheme's and its variant MUST be valid.

### 3.4 The registry

- A process holds at most one scheme per domain, in a process-wide registry. The store finds a key's scheme by the key's domain.
- Registering a scheme **identical** to the one already registered for its domain is a no-op that returns the registered one. Identical means: the same version, the same forms in the same order, the same closed variant set or the same pattern text, and the same dtype map.
- Registering a **different** scheme for a registered domain is a loud path (§1.6).
- Python conforms since pycommonkit 0.1.0 (A3). It used to compare only the variant tuples of two pattern schemes, which are both empty, so two different patterns compared as identical (vector `keys.registry.other_pattern`).

### 3.5 Readers and publishers (C4)

- A **publisher** MUST validate a key against the registered scheme for its domain (§3.3). A key that fails is counted `shm_bad_key` and declined (§6.2).
- A **reader** MAY attach any key that satisfies the grammar (§3.1), registered or not. A reader that also checks the scheme (Python does) conforms; it counts a key that fails as `shm_bad_key`.

### 3.6 Key to path

A key's **relative path** is

```text
{form}/{digest[0:2]}/{digest}.{variant}.npy
```

with `/` as the separator on POSIX systems (Windows: §12). It is never absolute, and it depends only on the key, not on the scheme. The vectors in `keys.json` (`parse`, field `relpath`) pin it.

### 3.7 Digests

The contract names only 32 lower-case hexadecimal digits. Which algorithm produced them (md5 today) is the scheme's business. A domain that changes algorithm MUST use a new domain or version, so that one key never names two contents (W1).

## 4. Layout

A root holds only these entries. Paths are relative to the root.

| Path | What it is | Who touches it |
|---|---|---|
| `{form}/{xx}/{digest}.{variant}.npy` | A segment; `xx` is `digest[0:2]` | Publish (rename-over), touch, reap (unlink) |
| `{form}/{xx}/*.tmp` | A segment being written (§6.5) | Its writer; the reaper sweeps orphans (§9.5) |
| `.reap.lock` | The reaper's lock (§9.2) | Whichever reaper holds it |
| `.usage.json`, `.usage.json.tmp` | The last complete walk (§9.7) | The reaper |
| `.writable.{pid}.{tid}` | A store's writability probe (§6.1) | Its process, briefly |
| `.policy.json` | Tree-wide policy (§10.5) | The maintainer, or a setup command |
| Any other name starting with `.` | Reserved: not a segment, not counted, never reaped | Its owner (PlumCraft's `.hello/` and `.sources/` today) |

- A **segment directory** is a directory `{form}/{xx}` where `form` is a token and `xx` is two characters from `[0-9a-f]`.
- No form can collide with a reserved name, because tokens cannot start with `.`.
- The root MUST be on a local filesystem. Network filesystems are not supported: their rename, unlink and mmap semantics are not those §7 relies on.
- Anything else under the root is outside the contract. The reaper leaves it alone (§9.4), but nothing promises it survives.

## 5. The `.npy` segment

A segment is a file in numpy's `.npy` format, exactly as `np.save(fh, arr, allow_pickle=False)` writes it. This section specifies that format so that it can be written and read without numpy.

### 5.1 The prefix

| Bytes | Content |
|---|---|
| 0 to 5 | The magic string `\x93NUMPY` |
| 6 | Major version: 1, 2 or 3 |
| 7 | Minor version: 0 |
| 8 to 9 (version 1) | `HLEN`, the header length, little-endian unsigned 16-bit |
| 8 to 11 (versions 2 and 3) | `HLEN`, little-endian unsigned 32-bit |

The prefix length `L` is 10 for version 1 and 12 for versions 2 and 3. The header text occupies bytes `L` to `L + HLEN - 1`; it is Latin-1 for versions 1 and 2 and UTF-8 for version 3. The **data offset** is `L + HLEN`, and the payload starts there.

### 5.2 The header text, as written

A writer MUST produce the following bytes, which are numpy's, exactly. Conformance means the same bytes for the same dtype, order and shape (C12); the vectors in `headers.json` pin them.

1. **The dict literal** `T`:

   ```text
   {'descr': D, 'fortran_order': F, 'shape': S, }
   ```

   - `D` is the descriptor as Python's `repr` writes it (§5.4).
   - `F` is `True` or `False` (§5.3).
   - `S` is Python's `repr` of the shape tuple: `()` for zero dimensions, `(n,)` for one, `(a, b, c)` for more; decimal integers with no sign or leading zeros, separated by `, `.
   - The keys appear in this order, each followed by `, `, then `}`.
2. **Growth-axis padding:** `G` spaces, where `G = max(0, 21 - len(str(g)))` and `g` is the **growth axis** length: `shape[0]` when `F` is `False`, `shape[-1]` when it is `True`. `G = 0` for a zero-dimensional array. (21 is numpy's `GROWTH_AXIS_MAX_DIGITS`.)
3. **Alignment padding:** `P` spaces, where `P = 64 - ((L + len(T) + G + 1) mod 64)`, with `len(T)` counted in encoded bytes. So `1 <= P <= 64`: a header that would end exactly on a 64-byte boundary gets a full 64 spaces, never none (vector `headers.align_full`).
4. A final `\n`.

So `HLEN = len(T) + G + P + 1`, and the data offset `L + HLEN` is a multiple of 64.

**Version choice.** A writer uses version 1 if `T` is Latin-1-encodable and `HLEN` (computed with `L = 10`) fits in 16 bits; otherwise version 2 if `T` is Latin-1-encodable (recomputing `P` with `L = 12`); otherwise version 3. In practice version 2 needs a header over 65535 bytes (a structured dtype with thousands of fields; vector `headers.v2`) and version 3 a field name outside Latin-1 (vector `headers.v3`).

Leaving out the growth-axis padding still makes a valid file, but not numpy's bytes: for a 20-dimensional shape the data offset is 192 with it and 128 without (vectors `headers.ones_15d` to `headers.ones_63d` carry both, as `data_offset` and `unpadded_data_offset`).

### 5.3 Memory order

- The payload holds the array's elements in C order when `fortran_order` is `False` and in Fortran order when it is `True`.
- A writer MUST write `True` only when the array is Fortran-contiguous and **not** also C-contiguous, as numpy does. An array with no elements, or with at most one dimension longer than 1, is both, so it is written `False` whatever order it was given in (vectors `headers.fortran_1d`, `headers.fortran_degenerate`, `headers.growth_f_empty`). A non-contiguous array is written in C order.

### 5.4 Descriptors

The `descr` value names the dtype.

- **Simple descriptors** are a string of a byte-order character, a kind and a size:
  - byte order: `<` little-endian, `>` big-endian, `|` not applicable. numpy writes `|` for one-byte kinds (`b1`, `i1`, `u1`), and for `S` and `V`; `<` or `>` otherwise.
  - kinds and sizes: `b1` (bool); `i1`, `i2`, `i4`, `i8`; `u1`, `u2`, `u4`, `u8`; `f2`, `f4`, `f8`; `c8`, `c16`; `M8[unit]` and `m8[unit]` (datetime, timedelta; the unit is optional); `Un` (UCS-4 text, item size `4n`); `Sn` (bytes); `Vn` (opaque bytes).
  - Long-double kinds (`f16`, `c32`) are platform-dependent and outside version 1: a reader MAY miss them.
- **Structured descriptors** are a list of fields. Each field is a tuple `(name, descr)` or `(name, descr, shape)`, where `descr` is simple or itself structured and `shape` is a subarray shape. numpy names padding fields `''`, so the item size is the sum over fields of the field's item size times the product of its subarray shape.
- **Writing** `D` uses Python's `repr`: a simple descriptor is written in single quotes (`'<f4'`); a structured one as a list of tuples (`[('a', '<i4'), ('b', '<f8')]`). A field name is written in single quotes unless it contains `'` and no `"`, in which case it is written in double quotes (vector `headers.struct_quote`). A writer that cannot reproduce Python's `repr` of a field name (one holding a backslash or a non-printable character) or a field title (numpy writes `((title, name), descr)`) MUST decline the array (`shm_declined_dtype`, §6.3).
- **Typed classes.** A reader offers typed access to `b1`, `i1` to `i8`, `u1` to `u8`, `f2`, `f4` and `f8`, in either byte order. Every other descriptor (complex, datetime, `U`, `S`, `V`, structured) is exposed as raw payload bytes plus the descriptor text.
- **Object dtypes** (`|O`, or any structured dtype with an object field; and numpy's variable-width `StringDType`, which also needs pickling) are never written (§6.3) and never read (§5.7).

### 5.5 Limits

- **Header length.** A reader MUST accept a header of up to 1 MiB (`HLEN <= 2^20`) and MUST treat a larger `HLEN` as torn without reading the text. A publisher MUST decline an array whose header would exceed it (`shm_declined_dtype`).
  - Python conforms since pycommonkit 0.1.0 (A4): it passes `max_header_size` of 1 MiB to `np.load`, checks the prefix's length field before numpy reads the text, and declines a larger header on publish. `np.load` used to refuse any header over 10000 bytes, so Python missed every version 2 segment, and version 1 segments with long headers (vectors `reads.header_v1_large`, `reads.header_v2`), while publishing them.
- **Dimensions.** A reader MUST accept up to 32 dimensions; numpy 1.x allows no more. numpy 2 writes up to 64, and a reader SHOULD accept those too.
- **Size.** A reader MUST compute `itemsize × product(shape)` with checked arithmetic and MUST treat an overflow as torn.

### 5.6 Reading the header

A reader MUST accept every header a conforming writer produces (§5.2), and in addition:

- any length of space padding before the final `\n`, so headers aligned to 16 bytes, as numpy before 1.9 wrote them, are read (vector `reads.align_16`). A reader MUST NOT require the data offset to be aligned; it MAY copy rather than map an unaligned payload for typed access;
- versions 1, 2 and 3 (minor 0).

A reader MAY accept other Python-literal spellings that numpy's own reader accepts (keys in another order, double-quoted strings, other whitespace). No conforming writer produces them, and no vector depends on them.

### 5.7 Whole, torn and trailing

A file at a segment path is **torn** when any of these holds, and a reader MUST then miss and count `shm_torn` (§8.1):

- it is shorter than its prefix, or than `L + HLEN`;
- the magic is wrong, or the version is not 1.0, 2.0 or 3.0;
- `HLEN` exceeds the limit of §5.5;
- the header text does not decode, is not a dict literal, or does not have exactly the keys `descr`, `fortran_order` and `shape`;
- `descr` is not a descriptor of §5.4, or contains an object dtype (a reader MUST NOT unpickle);
- `fortran_order` is not `True` or `False`;
- `shape` is not a tuple of non-negative integers;
- the payload size overflows (§5.5);
- the file is shorter than `data offset + itemsize × product(shape)` (**short payload**).

Otherwise the file is **whole**. Bytes after the payload are **trailing bytes**: a reader MUST ignore them and attach the array (C6(d); numpy does; vectors `reads.trailing_5`, `reads.trailing_copy`).

### 5.8 Structured padding bytes

The payload of a structured dtype with padding includes the padding bytes. In a descriptor, padding appears as fields with an empty name (`('', '|V3')`); when a descriptor is read, those become padding, not fields. The **padding bytes** of an item are the bytes no named field covers, recursively through nested structured fields.

- Writers write padding bytes as they hold them (numpy does). Their values carry no meaning.
- A copying read in Python (`np.load` without a mapping, `attach="read"`) zeroes them, so an array read back by copy is not byte-identical to its file for such a dtype, although every field value is the same.
- So padding bytes MUST NOT decide equality: `verify_republish` (§6.4) compares named-field bytes only (vectors `existing_key.padded_*`).

## 6. Publishing

### 6.1 The store

A store has a root, a **mode** (`off`, `read` or `readwrite`), an **attach mode** (`mmap`, which maps the file read-only, or `read`, which copies it), and the policy of §10.

- Construction MUST NOT fail. The store is **disabled** when the mode is `off`, the root is empty or `/`, the root cannot be created, or a probe file `.writable.{pid}.{tid}` cannot be created and written in it. The probe is then unlinked; failing to unlink it does not disable the store.
- A disabled store attaches nothing (a miss with no counter), declines every publish (`shm_declined_mode`), and does not reap.

### 6.2 Gates

`publish(key, array)` checks these gates **in this order**. The first that fails declines the publish, increments its counter, and returns "not stored". A decline is normal operation.

| # | Gate | Fails when | Counter |
|---|---|---|---|
| 1 | Mode | The store is disabled, or its mode is not `readwrite` | `shm_declined_mode` |
| 2 | Small | `nbytes < min_array_bytes` | `shm_declined_small` |
| 3 | Size | `nbytes > max_array_bytes` | `shm_declined_size` |
| 4 | Space | Free bytes (§9.9) `< min_free_bytes`; then a rate-limited reap turn (§9.8) | `shm_declined_space` |
| 5 | Budget | `.usage.json` is fresh and valid (§9.7), and `bytes + nbytes > max_bytes`; then a rate-limited reap turn | `shm_declined_budget` |
| 6 | Dtype | §6.3 | `shm_declined_dtype` |
| 7 | Key | The key fails its registered scheme (§3.5) | `shm_bad_key` |

`nbytes` is `itemsize × product(shape)`. The limits themselves are admitted: `nbytes == max_array_bytes`, `nbytes == min_array_bytes`, and `bytes + nbytes == max_bytes` all pass (vectors `admission.size_equal`, `admission.small_equal`, `admission.budget_equal`).

### 6.3 Dtype admission

The dtype gate declines (`shm_declined_dtype`):

- **any object dtype** (§5.4), whatever the form (C6(c)). Python conforms since pycommonkit 0.1.0 (A6): it declines any dtype with `hasobject`, which covers `StringDType` too. `np.save(..., allow_pickle=False)` used to raise `ValueError`, which `publish` did not catch, and left the temporary file behind (vector `admission.free_object`);
- an array whose header would exceed the limit, or whose field names or titles the writer cannot reproduce (§5.4, §5.5);
- an array of a form whose scheme declares a dtype (§3.2), when the array's dtype is not **identical** to it. Identity is numpy dtype equality, which for the kinds a form can usefully declare is equality of canonical descriptors: `>f4` is not `<f4`, and `|i1` is not `|u1` (vectors `admission.tensor_f4_big`, `admission.img_i1`).

A form with no declared dtype admits any dtype that is not an object dtype. If the key's scheme cannot be found at this gate, the gate passes and gate 7 counts the bad key.

### 6.4 An existing key (C15)

After the gates and before writing, the publisher examines the segment path. It MUST NOT rewrite a whole segment.

1. **Absent:** write (§6.5).
2. **Torn** by the rules of §5.7: damage from outside the contract. Write the new segment over it (§6.5), and count `shm_torn_replaced` in addition to the counters of a publish (vectors `existing_key.torn_*`).
3. **Whole, with the same dtype, shape and fortran order** as the header the new array would be written with: a **republish**.
   - With `verify_republish` on (§10), first compare the existing payload, `nbytes` bytes from the data offset, with the new array's payload bytes; trailing bytes are not compared, and for a structured dtype with padding neither are the padding bytes (§5.8). If any compared byte differs, it is a collision (item 4).
   - Otherwise write nothing, touch the segment (§8.2), and return "stored". A republish increments no counter and takes no reap turn.
4. **Whole, with a different dtype, shape or order**, or different bytes under `verify_republish`: a **key collision**, a major system error (a key scheme or digest is broken).
   - Leave the existing file exactly as it is, including its mtime.
   - Count `shm_key_collision`, and log at error level.
   - Fail loudly: Python raises `KeyCollision`, a subclass of `ValueError`; Rust returns `Err(KeyCollision)`. This is the second loud path (§1.6).

Notes:

- "Same order" compares the `fortran_order` the new array would be written with (§5.3). The same values in another memory order are a different file, so they collide (vector `existing_key.other_order`).
- Without `verify_republish`, a collision with the same dtype, shape and order goes unseen and is treated as a republish (vector `existing_key.same_header_other_bytes`). The conformance suite and CI run with it on.
- **Read back by copy:** a structured array with padding that was read by copy in Python has zeroed padding bytes (§5.8). Republishing it is a republish, not a collision, because padding bytes are never compared (vectors `existing_key.padded_padding_differs_verified`, and `existing_key.padded_field_differs_verified` for a real difference).
- Two publishers racing on one absent key may both write and both rename. By W1 they hold the same content, so the result is the same.
- An existing path that is not a regular file is outside the contract.
- Python conforms since pycommonkit 0.1.0 (A7). It used to rewrite an existing key unconditionally: it counted a publish, never touched, never detected a collision, and had neither counter nor `verify_republish` (vectors `existing_key.*` except `absent`). `TieredPixelCache`, which catches every other exception from `publish`, lets `KeyCollision` through.

### 6.5 Writing

1. Create the segment directory if needed.
2. Write the whole file (§5) to a **temporary file** in the segment's own directory. Its name MUST end in `.tmp`, MUST NOT end in `.npy`, and MUST be unique to the writing process and thread. Python and Rust use `{segment file name}.{pid}.{thread id}.tmp`.
3. If `fsync` is on (§10): sync the temporary file's data to storage.
4. Rename the temporary file over the segment path. Rename is atomic within one filesystem, which is why the temporary file lives in the same directory.
5. If `fsync` is on: sync the segment directory, so the rename survives a crash.

Errors:

- ENOSPC or EDQUOT at any step before the rename counts `shm_declined_space`; any other error counts `shm_publish_failed`. Either way the temporary file is removed and the publish returns "not stored".
- A failed sync before the rename is such an error. A failed directory sync after the rename counts `shm_publish_failed` and returns "not stored"; the segment, which is whole, is left in place.
- `fsync` is off by default. With it off a crash can leave a short file under the segment name, which readers treat as torn (W4) and publishers replace (§6.4).
- Python conforms since pycommonkit 0.1.0 (A8). On macOS it syncs with `F_FULLFSYNC`, since `fsync` there does not flush the drive's cache; on Windows, which cannot open a directory to sync it, step 5 is skipped (§12).

### 6.6 After a successful write

Count `shm_publish` and add `nbytes` to `shm_bytes_published`, then take a rate-limited reap turn (§9.8). Return "stored". Publishers in every language reap (C9).

## 7. The mutation rule

This rule is what makes mapping safe.

A segment path is changed only by:

- **rename-over**, when it is published (§6.5);
- **unlink**, by the reaper (§9.6), or by a writer removing its own temporary file;
- **an mtime update**, by a touch (§8.2).

No conforming process truncates a segment, writes into it in place, or opens it for writing.

On POSIX systems unlinking or renaming over a file that another process has mapped is safe: the mapping keeps the old inode, and the kernel reclaims it on the last unmap. Truncating a mapped file is not safe: a reader touching a page past the new end receives SIGBUS, which kills the process and cannot be caught in Rust. So "unlink, never truncate" (W3) is a contract rule, not an implementation detail. Tests that truncate an unmapped file to make a torn one are a test-only exception. Windows behaves differently (§12).

## 8. Reading and touch

### 8.1 Attach

`attach(key)` returns an array or a miss:

| Condition | Result | Counter |
|---|---|---|
| The store is disabled | Miss | None |
| The key fails the reader's check (§3.5) | Miss | `shm_bad_key` |
| No file at the path | Miss | `shm_miss` |
| The file is torn (§5.7), or opening, mapping or reading it fails for any other reason | Miss | `shm_torn` |
| The file is whole | The array, read-only | `shm_hit`, then a touch (§8.2) |

- With attach mode `mmap` the array MUST be a read-only mapping; with `read` it is a private copy. A copy counts as a hit and touches too.
- Every failure of parsing, mapping or reading MUST be caught and counted. Python conforms since pycommonkit 0.1.0 (A5): `attach` catches every exception but a missing file as torn. numpy raises `tokenize.TokenError` for some unparseable headers, which `attach` used to let escape to the caller (vector `reads.garbage_text`).

### 8.2 Touch

The LRU order is mtime. On every hit, and on every republish (§6.4), the process sets the segment's mtime (and atime) to now **if** its mtime is more than 60 seconds old (`TOUCH_INTERVAL_S`), so a segment gets at most one metadata write per minute. A touch that fails, for example on a file owned by another user, is ignored.

atime is never used: ZFS serving reads from ARC does not update it. Readers in every language MUST touch (plan inconsistency 1).

## 9. The reaper

The reaper is the only thing that removes segments. It runs on the publisher's schedule (§9.8) or when called directly with a time budget `budget_s`.

### 9.1 Refusals

- A disabled store does not reap.
- The reaper MUST refuse to run, counting `shm_reap_refused`, when the root is empty, is `/`, or contains fewer than two path separators (so `/tmp` is refused and `/tmp/store` is not).

### 9.2 The lock

- The reaper creates `.reap.lock` with `O_CREAT | O_EXCL` and mode 0644, then writes its pid into it as decimal text.
- If the lock exists: when its mtime is more than 300 seconds old, the reaper unlinks it; either way it returns **without reaping** on this turn. Any other failure to create the lock also returns without reaping.
- Every exit after acquiring the lock releases it by unlinking it.
- Two reapers may still race after a stale lock is broken. This is harmless: unlinking twice fails with ENOENT, which is skipped, and an interleaved `.usage.json` either parses or is treated as unknown (§9.7). (C8: kept for version 1.)

### 9.3 The walk

- The reaper takes `now` once, then walks the whole root without following symbolic links, `lstat`ing each candidate.
- The deadline `start + budget_s` is checked **before each directory**. A walk stopped by the deadline is **partial**; one that visits every directory is **complete**. `budget_s` limits the walk, never the eviction (§9.6).
- A reaper MAY skip descending into `.`-prefixed directories, since nothing in them is a candidate.

### 9.4 Candidates (C6(a))

A **candidate** is a regular file (by `lstat`) whose path relative to the root matches, in full:

```text
([a-z0-9]+)/([0-9a-f]{2})/(\2[0-9a-f]{30})\.([a-z0-9]+)\.npy
```

That is, the segment shape of §4 with the shard directory equal to the digest's first two characters. Nothing else under the root is ever unlinked by the reaper except orphaned temporary files (§9.5).

Python conforms since pycommonkit 0.1.0 (A9). Its reaper used to treat any regular file ending in `.npy` anywhere under the root as a candidate (vectors `reaper_paths.shard_mismatch`, `too_shallow`, `too_deep`, `root_npy`, `no_variant`, `upper_digest`, `dot_dir`).

### 9.5 Orphaned temporary files (C6(b))

During the walk the reaper unlinks every regular file in a segment directory (§4) whose name ends in `.tmp` and whose mtime is more than 3600 seconds before `now`. Such a file is left by a writer that crashed. Swept files are not candidates: they are not counted in `scanned`, in usage, or in `shm_reaped`. Temporary files anywhere else (the root, `.`-prefixed directories) are left alone.

Python conforms since pycommonkit 0.1.0 (A9; vectors `reaper_paths.tmp_old`, `tmp_dot_old`).

### 9.6 Order and eviction

- Candidates are sorted by `(mtime, size, path)`, oldest first. `total` is the sum of their sizes.
- `low_water = floor(0.8 × max_bytes)`.
- `need_free = max(0, floor(1.25 × min_free_bytes) − free)`, with `free` measured fresh (§9.9), not from the cache.
- Walking the sorted list, with `freed` starting at 0, the reaper unlinks the candidate while **any** of these holds:
  - **expired:** `now − mtime > ttl_s`;
  - **over:** the walk was complete and `total > low_water`;
  - **short:** `freed < need_free`.
- It stops at the first candidate for which none holds. An unlink that fails is skipped. Each success subtracts the size from `total` and adds it to `freed`.

### 9.7 Output and `.usage.json`

- The reaper counts `shm_reaped` (segments unlinked) and `shm_reaped_bytes` (their sizes), and reports `scanned`, `reaped`, `bytes` and `complete`.
- **After a complete walk only**, it writes `.usage.json` as a JSON object `{"bytes": B, "files": N, "ts": T}`: `B` is `total` after eviction (an integer), `N` is candidates scanned minus reaped (an integer), and `T` is `now` in Unix seconds (a number). It writes `.usage.json.tmp` and renames it over `.usage.json`. A partial walk writes nothing and counts `shm_reap_partial`, because it would under-count.
- The reaper invalidates the store's cached free-space figure.
- **Reading** `.usage.json` (the budget gate, §6.2): it is **fresh** when `now − its mtime <= 2 × reap_interval_s`. It is **valid** when it is a JSON object whose `bytes` is a non-negative integer. A missing, stale or invalid file means usage is unknown, and the budget gate passes. Python conforms since pycommonkit 0.1.0 (A11). It used to accept a numeric string or a float for `bytes` too.

### 9.8 The rate limit

A store keeps the time of its last reap turn, initially zero. A **reap turn** (after a successful publish, and after a space or budget decline) does nothing if `now − last < reap_interval_s`; otherwise it sets `last = now` and reaps with `budget_s = 0.05`. Any failure inside a turn is caught and counted `shm_reap_failed`.

### 9.9 Free space

Free bytes are `f_bavail × f_frsize` from `statvfs` on the root, cached for one second per store. If the figure cannot be obtained for any reason, free bytes are 0. (C6(e). Python conforms since pycommonkit 0.1.0 (A10). It used to catch only `OSError`, so a platform without `os.statvfs` raised `AttributeError`. Where `os.statvfs` is missing it now asks `shutil.disk_usage`, whose POSIX branch is the same figure.)

## 10. Policy

### 10.1 Settings

Tree-wide policy applies to every segment in a tree, whichever process reaps, so it comes only from the policy file, the environment and the defaults, never from a caller. A caller chooses only its mode, its attach mode and its `min_array_bytes` (256 KiB by default for the shared store). Tests MAY pass an explicit root.

| Setting | Environment variable | Kind | macOS default | Linux default | Store field |
|---|---|---|---|---|---|
| `ROOT` | `COMMONKIT_STORE_ROOT` | path | `~/Library/Caches/commonkit/store` | `/dev/shm/commonkit` | `root` |
| `MAX_GB` | `COMMONKIT_STORE_MAX_GB` | number | 2.0 | 8.0 | `max_bytes = trunc(MAX_GB × 2^30)` |
| `MIN_FREE_GB` | `COMMONKIT_STORE_MIN_FREE_GB` | number | 8.0 | 8.0 | `min_free_bytes = trunc(MIN_FREE_GB × 2^30)` |
| `TTL_S` | `COMMONKIT_STORE_TTL_S` | number | 900 | 900 | `ttl_s` |
| `REAP_S` | `COMMONKIT_STORE_REAP_S` | number | 30 | 30 | `reap_interval_s` |
| `MAX_ARRAY_BYTES` | `COMMONKIT_STORE_MAX_ARRAY_BYTES` | number | 134217728 | 134217728 | `max_array_bytes = trunc(MAX_ARRAY_BYTES)` |
| `FSYNC` | `COMMONKIT_STORE_FSYNC` | flag | off | off | `fsync` |
| `VERIFY_REPUBLISH` | `COMMONKIT_STORE_VERIFY_REPUBLISH` | flag | off | off | `verify_republish` |

- "Linux" means every non-macOS POSIX system. Windows: §12.
- `trunc` truncates toward zero. Values are not range-checked in version 1: a negative TTL, for example, is accepted.
- `FSYNC` and `VERIFY_REPUBLISH` are new. Python conforms since pycommonkit 0.1.0 (A13).
- On thresher the tree's policy is `ROOT=/Volumes/CommonkitStore/store`, `MAX_GB=7`, `MIN_FREE_GB=1`, `TTL_S=604800`.

### 10.2 Numbers

A number value, from the environment or a policy-file string, is **valid** when, after removing leading and trailing ASCII whitespace (space, tab, LF, VT, FF, CR), it matches in full

```text
[+-]?([0-9]+(\.[0-9]*)?|\.[0-9]+)([eE][+-]?[0-9]+)?
```

and its value as an IEEE 754 double (round to nearest) is finite. For the byte-count settings the derived integer (`max_bytes`, `min_free_bytes`, `max_array_bytes`) must also lie in `[−2^63, 2^63 − 1]`. Anything else is **malformed**. An unset, empty or malformed value is ignored, and the next source applies (§10.6); it never raises.

Python conforms since pycommonkit 0.1.0 (A12). It used to parse with `float()`, which also accepts `inf`, `nan`, `1_000`, non-ASCII digits and Unicode whitespace; and `int(float("inf"))` raised `OverflowError` (vectors `env.ttl_inf`, `env.ttl_underscore`, `env.ttl_arabic_digits`, `env.ttl_nbsp`, `env.max_array_inf`, `env.max_gb_huge`).

### 10.3 Flags

A flag value is **on** when it equals `1`, `true`, `yes` or `on`, compared case-insensitively over ASCII, with nothing before or after it. Any other value, including an empty one, ` 1` and `true ` with spaces, `2` and `y`, is **off**. An unset flag is off. (Vectors `env.fsync_*`, `env.verify_republish_*`.)

### 10.4 The root

- The root comes from `COMMONKIT_STORE_ROOT` when it is set and non-empty, otherwise from the platform default. It never comes from the policy file, because the root is how a process finds that file.
- A leading `~`, alone or followed by `/`, expands to the user's home directory. (Python's `expanduser` also expands `~user`; that form is outside the contract.)
- A relative root is resolved against the working directory.
- An explicit root given by a caller (tests) overrides both.

### 10.5 The policy file (C7)

- **Location:** `.policy.json` directly under the root. It is read once, when the shared store is constructed. A store constructed directly with explicit settings (tests) is configured by its caller.
- **Validity:** the file is used only when it is a regular file of at most 65536 bytes, holding UTF-8 JSON (RFC 8259) whose top level is an object with no duplicate keys. `NaN` and `Infinity` are not JSON. A missing, oversized, unreadable or invalid file is treated as **absent**, entirely.
- **Keys** are the setting names of §10.1 exactly, with no `COMMONKIT_STORE_` prefix and in upper case. `ROOT` is ignored (§10.4). Unknown keys are ignored.
- **Values:**
  - a number setting takes a JSON number, or a string valid under §10.2;
  - a flag takes JSON `true` or `false`, or a string read under §10.3;
  - any other value (`null`, a boolean for a number, a number for a flag, an array, an object, a malformed string) is ignored for that key, which the file then **leaves out**.
- Writers of the file (the maintainer, or a setup command) SHOULD write it by temp-then-rename.
- Python conforms since pycommonkit 0.1.0 (A14). It used to read no policy file (vectors `policy.*`).

### 10.6 Precedence

For each setting except `ROOT`, the effective value is the first of:

1. the policy file's value, when the file is valid and the key is present with a valid value;
2. the environment's value, when it is set and valid;
3. the platform default.

So the file wins over the environment, and the environment fills only what the file leaves out (C7).

## 11. Counters

Implementations use these names, so a stats table can sum them across languages. Each is incremented only where the section given says.

| Counter | Section |
|---|---|
| `shm_hit`, `shm_miss`, `shm_torn` | §8.1 |
| `shm_bad_key` | §6.2, §8.1 |
| `shm_torn_replaced` (new; Python conforms since pycommonkit 0.1.0, A7) | §6.4 |
| `shm_key_collision` (new; Python conforms since pycommonkit 0.1.0, A7) | §6.4 |
| `shm_declined_mode`, `shm_declined_small`, `shm_declined_size`, `shm_declined_space`, `shm_declined_budget`, `shm_declined_dtype` | §6.2, §6.5 |
| `shm_publish`, `shm_bytes_published` | §6.6 |
| `shm_publish_failed` | §6.5 |
| `shm_reaped`, `shm_reaped_bytes`, `shm_reap_partial` | §9.7 |
| `shm_reap_refused` | §9.1 |
| `shm_reap_failed` | §9.8 |

A store's counters are drained (read and reset together) by `drain_stats`.

## 12. Windows

Windows is a supported platform (C10). Commonkit work for Windows already exists and will be merged later; this section will be completed then. Nothing in this contract may be read as excluding Windows. What is known now:

- **Windows cannot unlink or rename over a file that is open or mapped** (without `FILE_SHARE_DELETE` on every handle, and mapping still pins it). This touches:
  - the mutation rule (§7), whose safety argument is POSIX's;
  - the reaper (§9.6), whose unlink of a mapped segment fails rather than succeeding;
  - rename-over when publishing and when replacing a torn segment (§6.4, §6.5).
- **There is no `statvfs`.** How free space is measured (§9.9) is not yet specified.
- **The default root is unspecified.** ImpCraft's existing reader uses `%LOCALAPPDATA%\commonkit\store`; that is existing practice, not yet contract. Python today would use `/dev/shm/commonkit`.
- The path separator in §3.6 and the separator count in §9.1 are POSIX rules.

Until this section is complete, Windows behaviour is unspecified, and the vectors give `null` where an expectation is platform-specific to Windows.

## 13. Conformance vectors

### 13.1 Layout and regeneration

`conformance/vectors/` holds JSON vector files and small fixtures; `conformance/generate.py` writes all of them from numpy, deterministically:

```sh
uv run python conformance/generate.py
git diff --exit-code conformance/
```

The second command must be clean. The fixtures are identical whether written by numpy 1.26, 2.1 or 2.4.

Every vector file is a JSON object with `contract` (1), `category`, `about`, and its vectors. Every vector has an `id` (unique within its list), MAY have a `note`, and carries `"python": "step3"` when Python does not yet conform (§1.3). Paths in vectors are relative to `conformance/vectors/`.

### 13.2 Array specs

Several files describe an array as `{"descr", "shape", "order", "fill"}`:

- `descr` is a simple descriptor written bare (`<f4`) or a structured one as its Python literal (`[('a', '<i4')]`); `fields` (`{"count", "digits", "descr"}`) abbreviates a structured dtype of fields `f0…` to `f{count−1}`, zero-padded to `digits`, each of `descr`;
- `order` is `C` or `F`; the payload bytes are laid out in that order;
- `fill` is `ramp` or `ramp2` (deterministic bytes, different in every position; see `_fill` in the generator), `zeros`, or `objects` (an object array of `None`).

### 13.3 The files

| File | Vectors | Expected outcome |
|---|---|---|
| `keys.json` | `build`: scheme, digest, variant, form. `parse`: a key and a scheme. `registry`: two schemes registered in turn under `domain`. Also `grammar` (§3.1) and `schemes` | `build`: `key`, or `error` (`digest`, `form`, `variant`). `parse`: `grammar` (bool), `scheme_ok`, and when ok `parts` and `relpath`. `registry`: `idempotent` or `error` |
| `headers.json` | A fixture under `npy/` and the `array` spec that produced it | Writers reproduce the file byte for byte; readers recover `version`, `header_len`, `data_offset`, `header_text`, `descr`, `fortran_order`, `shape`, `itemsize`, `payload_bytes`. Also `growth_padding`, `align_padding`, `unpadded_data_offset`, `sha256` |
| `reads.json` | A `key` and a `file` (or `null`: absent) placed at its path; run in both attach modes | `outcome`: `hit`, `miss` or `torn`; exact `counters`; for a hit `shape`, `descr`, `fortran_order`, `payload_sha256` |
| `admission.json` | A `key`, an `array`, `store` settings and an optional fresh `.usage.json` (`usage`) | `outcome`: `published`, `declined:<gate>` or `bad_key`; exact `counters`; no `.tmp` file left |
| `existing_key.json` | A `key`, an `existing` fixture (or `null`) aged `mtime_age_s`, the new `array`, and `verify_republish` | `outcome`: `published`, `touched` or `KeyCollision`; exact `counters`; `file_after` (`unchanged`, or `new` with `file_sha256`); `mtime` (`unchanged`, `refreshed` or `new`) |
| `reaper_paths.json` | A tree holding one entry at `path` (`kind` file or symlink), aged `age_s`, reaped with the settings in `reap` | `reaped` (with `shm_reaped` 1 and its bytes), `swept` (gone, no counter) or `kept` |
| `env.json` | `env` variables, no policy file | Effective store fields; `"default"` means the platform default in `defaults`; `{home}` and `{cwd}` are placeholders; `root` per platform where it differs |
| `policy.json` | A `policy_file` (`text`, a fixture `file`, or `directory`) and `env` | Effective store fields; `{root}` is the store's root |

"Exact counters" means the counters that changed, and by how much, with every other counter unchanged.

## Appendix A. Where Python changes in step 3

Each item is a clause above that was marked "Python: changes in step 3", with the vectors that test it. All are done. C6(d), trailing bytes, needs no change: Python already ignores them.

| # | Clause | Change | Vectors |
|---|---|---|---|
| A1 | §3.1 | **Done in pycommonkit 0.1.0.** Match keys, tokens and digests in full (no final newline) | `keys.build.digest_newline`, `keys.parse.trailing_newline` |
| A2 | §3.2 | **Done in pycommonkit 0.1.0.** Variant patterns match the whole variant, and the variant must be a token | `keys.build.pattern_prefix_only`, `keys.build.pattern_breaks_grammar`, `keys.parse.pattern_prefix_only` |
| A3 | §3.4 | **Done in pycommonkit 0.1.0.** Registry identity compares variant patterns | `keys.registry.other_pattern` |
| A4 | §5.5 | **Done in pycommonkit 0.1.0.** Read headers up to 1 MiB (`max_header_size`); decline larger ones on publish | `reads.header_v1_large`, `reads.header_v2` |
| A5 | §8.1 | **Done in pycommonkit 0.1.0.** `attach` catches every parse failure, `tokenize.TokenError` included | `reads.garbage_text` |
| A6 | §6.3 | **Done in pycommonkit 0.1.0.** Decline object dtypes as `shm_declined_dtype` (C6(c)); leave no temporary file | `admission.free_object` |
| A7 | §6.4 | **Done in pycommonkit 0.1.0.** The existing-key rule: replace torn, touch a republish, raise `KeyCollision`; counters `shm_torn_replaced`, `shm_key_collision`; option `verify_republish`, comparing named-field bytes only (§5.8) (C15). `cache.py`, which catches every other exception from `publish`, MUST (and does) let `KeyCollision` through | `existing_key.*` except `absent` |
| A8 | §6.5 | **Done in pycommonkit 0.1.0.** The `fsync` option (C17) | `env.fsync_*`, `policy.fsync_*` |
| A9 | §9.4, §9.5 | **Done in pycommonkit 0.1.0.** Reap only segment-shaped paths (C6(a)); sweep orphaned `.tmp` files over an hour old (C6(b)) | `reaper_paths.*` formerly marked step3 |
| A10 | §9.9 | **Done in pycommonkit 0.1.0.** Free space is 0 on any failure, `AttributeError` included (C6(e)) | None (Windows only) |
| A11 | §9.7 | **Done in pycommonkit 0.1.0.** `.usage.json` `bytes` must be a JSON integer | None |
| A12 | §10.2 | **Done in pycommonkit 0.1.0.** The number grammar: no `inf`, `nan`, `_`, non-ASCII digits or whitespace; derived byte counts in range | `env.ttl_*`, `env.max_array_inf`, `env.max_gb_huge` formerly marked step3 |
| A13 | §10.3 | **Done in pycommonkit 0.1.0.** The flags `FSYNC` and `VERIFY_REPUBLISH`, and store fields `fsync`, `verify_republish` | `env.fsync_*`, `env.verify_republish_*` |
| A14 | §10.5, §10.6 | **Done in pycommonkit 0.1.0.** `.policy.json`, file over environment (C7); `shared.py` stops saying policy comes "only" from the environment | `policy.*` formerly marked step3 |
