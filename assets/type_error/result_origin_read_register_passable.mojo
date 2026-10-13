# A read value of a trivially register-passable type is passed in registers:
# it has no memory origin at all.
# expect: value of type 'P' doesn't have a memory origin in origin specifier
@fieldwise_init
struct P(TrivialRegisterPassable):
    var x: Int


def alias(items: P) -> ref[origin_of(items)] P:
    return items


def main():
    print(alias(P(1)).x)
