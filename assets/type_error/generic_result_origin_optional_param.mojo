# `Optional` is `RegisterPassable` where its payload is; a conditional
# conformance that does not hold still might, so its origin is not one a
# result can name, even over a closed payload.
# expect: cannot return 'items's origin, because it might expand to a RegisterPassable type
def first(mut items: Optional[String]) -> ref[origin_of(items)] Optional[String]:
    return items


def main():
    print(1)
