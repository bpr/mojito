# A function a bracket specialized (`twice[4]`) binds a compile-time callable
# slot as upstream's symbol reference to that specialization: a callable
# parameter's argument and default, a struct's callable parameter, and a
# callable pack's element; a bracket over a static (`T.gst[3]`) alike. Its arguments may read the caller's binders, and
# two specializations name two instances.


def twice[n: Int]() -> Int:
    return n * 2


def run[f: def() thin -> Int]() -> Int:
    return f()


def run_default[f: def() thin -> Int = twice[4]]() -> Int:
    return f()


def run_keyword_default[f: def() thin -> Int = twice[n=4]]() -> Int:
    return f()


def run_earlier[k: Int, f: def() thin -> Int = twice[k]]() -> Int:
    return f()


def outer[k: Int]() -> Int:
    return run[twice[k]]()


def run_all[*fs: def() thin -> Int]() -> Int:
    var t = 0
    comptime for i in range(len(fs)):
        t += fs[i]()
    return t


struct S[f: def() thin -> Int]:
    @staticmethod
    def go() -> Int:
        return Self.f()


struct T:
    @staticmethod
    def gst[n: Int]() -> Int:
        return n + 1


def main():
    print(run_default())
    print(run_keyword_default())
    print(run[twice[4]]())
    print(outer[7]())
    print(run_earlier[5]())
    print(S[twice[3]].go() + S[twice[5]].go())
    print(run_all[twice[1], twice[2]]())
    print(run[twice[4]]() + run[twice[5]]())
    print(run[T.gst[3]]())
