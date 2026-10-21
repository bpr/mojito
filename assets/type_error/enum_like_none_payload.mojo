# expect: cannot get payload for None case
# `Optional`'s payload getter asserts at compile time that the case it reads
# carries a payload.


def main():
    var o = Optional(3)
    print(o._unsafe_get_enum_payload[0]())
