# expect: does not conform to trait 'ImplicitlyCopyable'
# An enum is `ImplicitlyCopyable` only when every payload is.
# Mojito spells the pin's `__enum` as `enum`, the spelling of upstream's
# enum proposal (`docs/non-goals.md`).


enum E(ImplicitlyCopyable):
    case list(List[Int])


def main():
    pass
