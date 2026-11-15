# expect: union case payload must not be 'NoneType'
# `NoneType` marks an `EnumLike` case without a payload, so it is no payload
# type; an empty payload is written `case nothing()`.
# `union` is the declaration upstream's enum proposal gives cases with
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


union E:
    case nothing(NoneType)


def main():
    pass
