# expect: enum inside a function not supported here
# An enum is declared at module scope only.
# `enum` is the declaration upstream's enum proposal gives cases without
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


def outer():
    enum Inner:
        case a


def main():
    pass
