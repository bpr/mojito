# A callable compile-time parameter its body never calls is removed, as on
# the pin: every binding shares one instance, a capturing closure's and a
# generic closure's included, and so does a function that only forwards the
# parameter to one that removed it. A body that calls it, directly or
# through a callee, keeps one instance per binding.
def ignore[f: def(Int) thin -> Int](x: Int) -> Int:
    return x + 1


def ignore_capturing[
    origins: OriginSet, //, f: def(x: Int) capturing[origins] -> Int
](x: Int) -> Int:
    return x + 2


def ignore_generic[
    origins: OriginSet, //, f: def[n: Int](Int) capturing[origins] -> Int
](x: Int) -> Int:
    return x + 3


def call[f: def(Int) thin -> Int](x: Int) -> Int:
    return f(x)


def forward_ignored[f: def(Int) thin -> Int](x: Int) -> Int:
    return ignore[f](x) * 10


def forward_called[f: def(Int) thin -> Int](x: Int) -> Int:
    return call[f](x) * 10


def forward_capturing[
    origins: OriginSet, //, f: def(x: Int) capturing[origins] -> Int
](x: Int) -> Int:
    var bump = 1
    return ignore_capturing[f](x) + bump


def forward_and_call[
    origins: OriginSet, //, f: def(x: Int) capturing[origins] -> Int
](x: Int) -> Int:
    return ignore_capturing[f](x) + f(x)


def double(x: Int) -> Int:
    return x * 2


def triple(x: Int) -> Int:
    return x * 3


def defaulted[f: def(Int) thin -> Int = double](x: Int) -> Int:
    return x + 4


def main():
    var k = 10

    @parameter
    def add_k(x: Int) -> Int:
        return x + k

    @parameter
    def sub_k(x: Int) -> Int:
        return x - k

    @parameter
    def add_kn[n: Int](x: Int) -> Int:
        return x + k + n

    @parameter
    def sub_kn[n: Int](x: Int) -> Int:
        return x - k - n

    print(ignore[double](1))
    print(ignore[triple](2))
    print(ignore_capturing[add_k](3))
    print(ignore_capturing[sub_k](4))
    print(ignore_generic[add_kn](5))
    print(ignore_generic[sub_kn](5))
    print(forward_ignored[double](1))
    print(forward_ignored[triple](1))
    print(forward_called[double](1))
    print(forward_called[triple](1))
    print(forward_capturing[add_k](1))
    print(forward_capturing[sub_k](2))
    print(defaulted(1))
    print(defaulted[triple](1))
    print(call[double](6))
    print(call[triple](7))
    print(forward_and_call[add_k](3))
    k = 20
    print(forward_and_call[sub_k](4))
