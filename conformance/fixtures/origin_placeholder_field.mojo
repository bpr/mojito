# An origin placeholder in a struct field position rejects on both compilers
# ("is not concrete" family; confirmed against the a79fbdf59f2 pin,
# 2026-08-29 — the pin says 'Span[Int, _]', Mojito 'Span[_]').
@fieldwise_init
struct Wrap:
    var inner: Span[Int, _]

def main():
    pass
