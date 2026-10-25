# expect: labeled payloads in enum cases are not supported
# An enum case's payload is a list of positional types, as upstream's
# `parseEnumCaseStmt` reads it: a labeled payload is rejected.
# Mojito spells the pin's `__enum` as `enum`, the spelling of upstream's
# enum proposal (`docs/non-goals.md`).


enum Point:
    case at(x: Int, y: Int)


def main():
    pass
