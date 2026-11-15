# expect: union cases must be declared separately, one per 'case'
# Each union case declares its own payload type, so cases are not listed
# on one line as an enum's are.
# `union` is the declaration upstream's enum proposal gives cases with
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


union Poll:
    case pending(), ready(Int)


def main():
    pass
