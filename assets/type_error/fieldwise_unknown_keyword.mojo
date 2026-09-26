# expect: unexpected keyword argument 'c'
# A fieldwise constructor's keywords are its field names; any other keyword
# matches no parameter.
@fieldwise_init
struct P(Copyable, Movable):
    var a: Int
    var b: Bool


def main():
    var p = P(a=1, c=True)
    print(p.a)
