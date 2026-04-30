# nix-log-check

*Check binary cache for possibly failing build logs*

## Nix flake

This repository is a Nix flake.
You can run `nix-log-check` with:

```console
$ nix run github:dramforever/nix-log-check
```

## Usage

Check the closure of a package to see why it needs building:

```console
$ nix-log-check nixpkgs#wireshark
```

Check your NixOS configuration:

```console
$ nix-log-check ".#nixosConfigurations.$(hostname).config.system.build.toplevel"
```

For more options:

```console
$ nix-log-check --help
```

## Requirements

Nix version 2.33 or later is required.
If you are on NixOS 25.11, you can use `nixVersions.nix_2_33`

## How does it work

`nix-log-check` checks a binary cache for derivations with a build log but without the outputs available.
It assumes that these are caused by the builds failing.

`nix-log-check` may miss some build failures if the build log was not submitted or has been garbage collected.
It may also produce false positives if the log was still available but the output was garbage collected.
However, it should work well for the common case, namely checking for build failures for a new update.
