# expect: enum methods must have a body; '...' is reserved
# An enum method has a body: upstream reserves a body of only `...`.
# `enum` is the declaration upstream's enum proposal gives cases without
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


enum E:
    case a

    def f(self) -> Int:
        ...


def main():
    pass
