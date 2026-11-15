# expect: enums cannot conform to 'TrivialRegisterPassable'
# An enum's storage is never trivially register-passable.
# `enum` is the declaration upstream's enum proposal gives cases without
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


enum E(TrivialRegisterPassable):
    case a


def main():
    pass
