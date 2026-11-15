# expect: enums may not have additional stored state
# An enum's only state is its active case, as upstream's enum proposal
# rules: shared state belongs in a struct holding the enum.
# `enum` is the declaration upstream's enum proposal gives cases without
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


enum E:
    case a
    var x: Int


def main():
    pass
