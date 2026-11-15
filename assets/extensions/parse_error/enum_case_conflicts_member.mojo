# expect: enum case 'red' conflicts with a member of the same name
# A case's generated member may not share its name with a written member.
# `enum` is the declaration upstream's enum proposal gives cases without
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


enum Color:
    case red

    def red(self) -> Int:
        return 1


def main():
    pass
