# expect: raising operation produces 'Opaque', but this context propagates 'Error'
# A typed raise converts to the context's `Error` only through one of
# `Error`'s `@implicit` initializers: a struct that is not `Writable` has
# none, and the pin rejects the call ("cannot call function that may raise
# 'Opaque' in context that supports an error type of 'Error'").
@fieldwise_init
struct Opaque(Movable):
    var code: Int


def fail() raises Opaque:
    raise Opaque(3)


def caller() raises:
    fail()


def main():
    try:
        caller()
    except e:
        print(e)
