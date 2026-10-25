# expect: case 'next' has recursive payload type 'Chain'
# A payload holding the enum itself by value would give the enum no size.
# Mojito spells the pin's `__enum` as `enum`, the spelling of upstream's
# enum proposal (`docs/non-goals.md`).


enum Chain:
    case end
    case next(Chain)


def main():
    pass
