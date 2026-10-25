# expect: enum case payload must not be 'NoneType'
# `NoneType` marks a case without a payload, so it is no payload type.
# Mojito spells the pin's `__enum` as `enum`, the spelling of upstream's
# enum proposal (`docs/non-goals.md`).


enum E:
    case nothing(NoneType)


def main():
    pass
