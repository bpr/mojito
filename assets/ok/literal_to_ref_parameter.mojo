# A literal passed to a `ref` parameter is materialized, and the parameter
# binds the temporary, as at the pin; so does one whose result names it.
def look(ref other: Int) -> Int:
    return other


def keep(ref other: Int) -> ref[other] Int:
    return other


def main():
    print(look(3))
    print(keep(9))
