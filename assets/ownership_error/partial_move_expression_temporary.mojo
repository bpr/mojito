# expect: field '(expression temporary).q.s' destroyed out of the middle of a value
@fieldwise_init
struct Q(Movable):
    var s: String

@fieldwise_init
struct P(Movable):
    var q: Q
    var n: Int

def mk() -> P:
    return P(Q(String("hi")), 1)

def main():
    var t = mk().q.s^
    print(t)
