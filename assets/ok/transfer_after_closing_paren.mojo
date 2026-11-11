# A postfix `^` abutting a closing parenthesis transfers the parenthesized
# expression: `(s)^` moves a `String` local, and `(lambda ...)^` moves a
# lambda's storage variable, which the binding would otherwise copy.
struct Token(Movable):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

def main():
    var s = String("hi")
    var t = (s)^
    print(t)
    var token = Token(4)
    var peek = (lambda {var token^} -> Int: token.id)^
    print(peek())
    var a = 6
    var b = 3
    print((a) ^ b)
