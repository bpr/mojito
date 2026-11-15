# expect: union case payloads do not support trait types; 'Movable' is a trait, use a concrete type or compile-time generic
# A case payload is a concrete type or a compile-time generic, never a
# trait, as the pin's declaration resolution rules.
# `union` is the declaration upstream's enum proposal gives cases with
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


union E:
    case a(Int, Movable)


def main():
    pass
