# expect: enum methods must have a body; '...' is reserved
# An enum method has a body: upstream reserves a body of only `...`.
# Mojito spells the pin's `__enum` as `enum`, the spelling of upstream's
# enum proposal (`docs/non-goals.md`).


enum E:
    case a

    def f(self) -> Int:
        ...


def main():
    pass
