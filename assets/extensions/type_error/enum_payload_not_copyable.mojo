# expect: case 'a' has non-'Copyable' payload type 'NC'
# A `Copyable` enum needs `Copyable` payloads. Declaring `ImplicitlyCopyable`
# asks `Copyable` first, so the pin names that trait for `NC` before it
# reaches `List[Int]`'s missing `ImplicitlyCopyable`.
# Mojito spells the pin's `__enum` as `enum`, the spelling of upstream's
# enum proposal (`docs/non-goals.md`).


struct NC(Movable):
    var x: Int


enum E(ImplicitlyCopyable):
    case a(NC)
    case b(List[Int])


def main():
    pass
