# expect: enum inside a function not supported here
# An enum is declared at module scope only.
# Mojito spells the pin's `__enum` as `enum`, the spelling of upstream's
# enum proposal (`docs/non-goals.md`).


def outer():
    enum Inner:
        case a


def main():
    pass
