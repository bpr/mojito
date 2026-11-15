# expect: case 'list' has non-'ImplicitlyCopyable' payload type 'List[Int]'
# A union is `ImplicitlyCopyable` only when every payload is, which the pin
# judges per case payload at the declaration.
# `union` is the declaration upstream's enum proposal gives cases with
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


union E(ImplicitlyCopyable):
    case list(List[Int])


def main():
    pass
