# expect: type 'Q' expects 0 type argument(s), got 1
# A construction's bracket binds the type's parameters, never the
# initializer's own, which are spelled after `.__init__` (`Q.__init__[7]()`):
# a bracket on a struct without parameters is an unexpected parameter.


struct Q:
    var a: Int

    def __init__[k: Int](out self):
        self.a = k


def main():
    print(Q[7]().a)
