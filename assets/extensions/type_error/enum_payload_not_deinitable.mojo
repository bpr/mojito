# expect: case 'a' has non-'Deinitable' payload type 'List[T]'
# Every case payload is `Deinitable`, whatever the enum declares: here the
# bound `Copyable` proves it for neither `T` nor `List[T]`.
# Mojito spells the pin's `__enum` as `enum`, the spelling of upstream's
# enum proposal (`docs/non-goals.md`).


enum E[T: Copyable]:
    case a(List[Self.T])


def main():
    pass
