# A `@fieldwise_init` constructor takes its fields by keyword, as the
# synthesized `__init__` names each parameter after its field: keywords bind
# in any order and mix with leading positionals, arguments evaluate in source
# order, and a variadic struct's pack is inferred or applied the same way.
@fieldwise_init
struct Rec(Copyable, Movable):
    var name: String
    var count: Int
    var tags: List[String]


@fieldwise_init
struct Tagged[T: Copyable & Movable & Deinitable](Copyable, Movable):
    var value: Self.T
    var flag: Bool


@fieldwise_init
struct Pair[*Ts: Copyable & Movable & Deinitable](Copyable, Movable):
    var storage: Tuple[*Self.Ts]


def tick(label: String, v: Int) -> Int:
    print("eval", label)
    return v


def tag[T: Copyable & Movable & Deinitable](var x: T) -> Tagged[T]:
    return Tagged(flag=True, value=x^)


def main():
    var name = String("alpha")
    var tags = List[String]()
    tags.append("x")
    var r = Rec(tags=tags^, count=tick("count", 3), name=name^)
    print(r.name, r.count, len(r.tags))
    var q = Rec("beta", tags=List[String](), count=tick("mixed", 7))
    print(q.name, q.count, len(q.tags))

    var t = tag(String("s"))
    print(t.value, t.flag)
    var u = Tagged[Int](flag=False, value=4)
    print(u.value, u.flag)

    var p = Pair[Int, Bool](storage=(1, True))
    print(p.storage[0], p.storage[1])
    var inferred = Pair(storage=(2, False))
    print(inferred.storage[0], inferred.storage[1])
