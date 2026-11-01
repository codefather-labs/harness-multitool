# Architecture

Example: replace it with the project's own layers.

```text
user -> interface -> core -> storage
```

## Main flow

1. A request comes in through the interface.
2. The core decides what to do.
3. The result is stored and returned.

## Layout

```text
src/    the code
docs/   this documentation
```
