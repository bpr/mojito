# expect: unions may not have additional stored state
# A union's only state is its active case's payload, as upstream's enum
# proposal rules: shared state belongs in a struct holding the union.
# `union` is the declaration upstream's enum proposal gives cases with
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


union WeirdResult:
    var timestamp: Int

    case success(Int)
    case failure(String)


def main():
    pass
