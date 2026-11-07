# A conditional expression whose register-passable result needs destroying
# yields that result as its `if`'s value, an owned value of its own: the
# binding or call taking it over keeps it alive up to there, and a result
# nothing takes over dies right after the conditional, as at the pin.


@fieldwise_init
struct R(Movable, RegisterPassable):
    var x: Int

    def __deinit__(deinit self):
        print("del", self.x)


def show(r: R):
    print("show", r.x)


def pick(c: Bool) -> R:
    var r = R(1) if c else R(2)
    print("picked", r.x)
    return r^


def main():
    var c = True
    var r = R(1) if c else R(2)
    print("got", r.x)
    _ = R(3) if not c else R(4)
    print("after discard")
    show(R(5) if c else R(6))
    var s = pick(False)
    print("s", s.x)

    var p = Optional[Int](1) if c else Optional[Int](4)
    print(p.value())
    var o: Optional[Int] = None
    var q = o or Optional[Int](4)
    print(q.value())
    var n: Optional[Int] = 1 if not c else None
    print(Bool(n))
    var m: Optional[Int] = None if not c else 1
    print(m.value())
    var v = 1 if c else Optional[Int](2)
    print(reflect[type_of(v)].name(), v.value())
