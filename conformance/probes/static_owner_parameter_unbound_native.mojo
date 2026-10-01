# PROBE: a generic struct's static that names the struct's parameter where no
# runtime argument or result carries it: an empty pack of `Self.T`, and a
# body-only use (`List[Self.T]()`).
#
# **Differs natively.** The pin and Mojito's VM print `0 0`. The native
# backend stops with "pliron backend: in `Pair.count`: unsupported
# unresolved type parameter `T`". Filed in `docs/roadmap.md` §2. When
# `run --backend pliron` prints `0 0`, promote this file to `assets/ok/`
# with its manifest rows.
#
# Observed 2026-09-30 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Run:    mojo run static_owner_parameter_unbound_native.mojo


struct Pair[T: Copyable & Deinitable]:
    @staticmethod
    def count(*values: Self.T) -> Int:
        return len(values)

    @staticmethod
    def made() -> Int:
        var items = List[Self.T]()
        return len(items)


def main():
    print(Pair[Int].count(), Pair[String].made())
