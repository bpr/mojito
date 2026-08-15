# expect: takes exactly 1
# A predicate-alias application binds arguments by position; a wrong count is
# a declaration-shaped error, not a silently false proposition.
comptime IsCopy[T: AnyType] = conforms_to(T, Copyable)

def pick[T: Copyable](value: T) -> T where IsCopy[T, Int]:
    return value

def main():
    print(pick(1))
