# A field destination: `b.s = String(b.s.rstrip())` views the field's owned
# bytes, which lie under the destination. The pinned Mojo rejects it with the
# same text.
# expect: aliasing values passed immutably to 'args' argument and constructed as a result in 'String' initializer call
struct Box:
    var s: String
    def __init__(out self, var s: String):
        self.s = s^

def main():
    var b = Box("abc  ")
    b.s = String(b.s.rstrip())
    print(b.s)
