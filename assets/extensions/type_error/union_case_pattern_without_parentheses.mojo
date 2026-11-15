# expect: union case 'pending' requires a payload pattern in parentheses
# A union case pattern always matches the payload: `.pending()` matches the
# empty one, and `.ready(_)` ignores one.
# `union` is the declaration upstream's enum proposal gives cases with
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


union Poll(Copyable):
    case pending()
    case ready(Int)


def main():
    var poll = Poll.pending()
    match poll:
        case .pending:
            pass
        case _:
            pass
