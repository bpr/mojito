# expect: invalid redefinition of 'a'
# Each case names a member of the enum, so two cases may not share a name.
# Mojito spells the pin's `__enum` as `enum`, the spelling of upstream's
# enum proposal (`docs/non-goals.md`).


enum E:
    case a
    case a


def main():
    pass
