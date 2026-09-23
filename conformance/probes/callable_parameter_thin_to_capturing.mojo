# Pin divergence probe (Mojo 1.2.0.dev2026092105): a module-level `def`
# handed to a runtime parameter of `def(...) capturing[_]` type. The pin
# rejects the call — "value passed to 'handler' cannot be converted from
# 'def show(element: Int) thin -> None' to 'def(element: Int) capturing thin
# -> None'" — and rejects a capturing closure there too ("capturing closures
# cannot be materialized as runtime values"), so such a parameter has no
# valid argument in the pin. Mojito converts the thin def and runs it.
# Roadmap section 3 carries the entry.
def apply(handler: def(element: Int) capturing[_], /):
    handler(1)


def show(element: Int):
    print(element)


def main():
    apply(show)
