# expect: case 'next' has recursive payload type 'Chain'
# A payload holding the union itself by value would give the union no size.
# `union` is the declaration upstream's enum proposal gives cases with
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


union Chain:
    case end()
    case next(Chain)


def main():
    pass
