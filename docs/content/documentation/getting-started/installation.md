+++
title = "Installation"
weight = 1
+++

Gutenberg provides pre-built binaries for MacOS, Linux and Windows on the
[GitHub release page](https://github.com/Keats/gutenberg/releases).

## Mac OS

Gutenberg is available on [Brew](https://brew.sh):

```bash
$ brew install gutenberg
```

## Windows

Gutenberg is available on [Scoop](http://scoop.sh):

```bash
$ scoop install gutenberg
```

## Arch Linux

Use your favourite AUR helper to install the `gutenberg-bin` package.

```bash
$ yaourt -S gutenberg-bin
```

## Void Linux

From the terminal, run the following command:

```bash
$ xbps-install -S gutenberg
```

## From source
To build it from source, you will need to have Git, [Rust and Cargo](https://www.rust-lang.org/)
installed (also few compiling tools & libraries : make, libsass, g++, libssl-dev)

From a terminal, you can now run the following command:

```bash
$ cargo build --release
```

The binary will be available in the `target/release` folder.
