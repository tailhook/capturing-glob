Capturing Glob
==============

[Documentation](https://docs.rs/capturing-glob) |
[Github](https://github.com/tailhook/capturing-glob) |
[Crate](https://crates.io/crates/capturing-glob)

Support for matching file paths against Unix shell style patterns, and
capture groups when matching (similarly to captures in regexes).


## Usage

And add this to your crate root:

```rust
extern crate capturing_glob;
```

## Examples

Print all jpg files in /media/ and all of its subdirectories.

```rust
use capguring_glob::glob;

for entry in glob("/media/**/(*).jpg").expect("Failed to read glob pattern") {
    match entry {
        Ok(entry) => println!("Path {:?}, name {:?}",
            entry.path().display(), entry.group(1).unwrap()),
        Err(e) => println!("{:?}", e),
    }
}
```
Note: in the case above, regular filename matching might be used
(i.e. ``file_stem()``), but the library allows you to skip unwraps here, but
more importantly you can use user-defined globs like these:

* ``(*)/package.json``
* ``tests/(*).spec.js``
* ``docs/(section-*).rst``
* ``/usr/share/zoneinfo/(*/*)``


License
=======

Licensed under either of

* Apache License, Version 2.0,
  (./LICENSE-APACHE or http://www.apache.org/licenses/LICENSE-2.0)
* MIT license (./LICENSE-MIT or http://opensource.org/licenses/MIT)
  at your option.

Contribution
------------

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.

