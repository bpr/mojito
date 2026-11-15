# expect: invalid redefinition of 'a'
# Each case names a member of the enum, so two cases may not share a name.
# `enum` is the declaration upstream's enum proposal gives cases without
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


enum E:
    case a
    case a


def main():
    pass
