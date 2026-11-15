# expect: case 'a' has non-'Deinitable' payload type 'List[T]'
# Every case payload is `Deinitable`, whatever the union declares: here the
# bound `Copyable` proves it for neither `T` nor `List[T]`.
# `union` is the declaration upstream's enum proposal gives cases with
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


union E[T: Copyable]:
    case a(List[Self.T])


def main():
    pass
