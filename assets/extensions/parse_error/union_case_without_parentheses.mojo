# expect: union case 'pending' must declare its payload in parentheses
# Every union case carries a payload, so it is always declared with
# parentheses; an empty list is the empty-tuple payload.
# `union` is the declaration upstream's enum proposal gives cases with
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


union Poll:
    case pending
    case ready(Int)


def main():
    pass
