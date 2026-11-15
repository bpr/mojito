# expect: case 'a' has non-'Copyable' payload type 'NC'
# A `Copyable` union needs `Copyable` payloads. Declaring `ImplicitlyCopyable`
# asks `Copyable` first, so the pin names that trait for `NC` before it
# reaches `List[Int]`'s missing `ImplicitlyCopyable`.
# `union` is the declaration upstream's enum proposal gives cases with
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


struct NC(Movable):
    var x: Int


union E(ImplicitlyCopyable):
    case a(NC)
    case b(List[Int])


def main():
    pass
