# expect: labeled payloads in union cases are not supported
# A union case's payload is a list of positional types, as upstream's
# `parseEnumCaseStmt` reads it: a labeled payload is rejected.
# `union` is the declaration upstream's enum proposal gives cases with
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


union Point:
    case at(x: Int, y: Int)


def main():
    pass
