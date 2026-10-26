# A display of one capturing lambda whose body writes its moved capture: the
# display's element is the lambda's closure storage, and each call through
# the element writes that storage's capture in place.
struct Counter(ImplicitlyCopyable, Movable, TrivialRegisterPassable):
    var n: Int
    def __init__(out self, n: Int):
        self.n = n
    def bump(mut self) -> Int:
        self.n += 1
        return self.n

def main():
    var c = Counter(4)
    var fs = [lambda {var c^} -> Int: c.bump()]
    print(fs[0](), fs[0]())
    var s = String("ab")
    var gs = [lambda {var s^} -> Int: (s.__iadd__("!"), s.byte_length())[1]]
    print(gs[0](), gs[0]())
