# expect: enum case 'empty' cannot carry a payload; declare a 'union' for cases with payloads
# An enum case never carries a payload, so it takes no parentheses, not
# even empty ones: a case with a payload is a union's.
# `enum` is the declaration upstream's enum proposal gives cases without
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


enum E:
    case empty()


def main():
    pass
