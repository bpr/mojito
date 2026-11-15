# expect: '_get_enum_discriminant' is synthesized for enums and cannot be declared
# The `EnumLike` members an enum gets are generated; writing one is an error.
# `enum` is the declaration upstream's enum proposal gives cases without
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


enum E:
    case a

    def _get_enum_discriminant(self) -> Int:
        return 0


def main():
    pass
