# expect: case 'list' has non-'ImplicitlyCopyable' payload type 'List[Int]'
# An enum is `ImplicitlyCopyable` only when every payload is, which the pin
# judges per case payload at the declaration.
# Mojito spells the pin's `__enum` as `enum`, the spelling of upstream's
# enum proposal (`docs/non-goals.md`).


enum E(ImplicitlyCopyable):
    case list(List[Int])


def main():
    pass
