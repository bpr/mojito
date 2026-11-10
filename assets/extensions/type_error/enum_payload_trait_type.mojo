# expect: enum case payloads do not support trait types; 'Movable' is a trait, use a concrete type or compile-time generic
# A case payload is a concrete type or a compile-time generic, never a
# trait, as the pin's declaration resolution rules.
# Mojito spells the pin's `__enum` as `enum`, the spelling of upstream's
# enum proposal (`docs/non-goals.md`).


enum E:
    case a(Int, Movable)


def main():
    pass
