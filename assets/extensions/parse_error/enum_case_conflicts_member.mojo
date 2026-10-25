# expect: enum case 'red' conflicts with a member of the same name
# A case's generated member may not share its name with a written member.
# Mojito spells the pin's `__enum` as `enum`, the spelling of upstream's
# enum proposal (`docs/non-goals.md`).


enum Color:
    case red

    def red(self) -> Int:
        return 1


def main():
    pass
