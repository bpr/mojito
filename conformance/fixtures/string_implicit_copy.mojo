# String is ImplicitlyCopyable: a place copies implicitly into owned positions.
def take(var text: String) -> Int:
    return text.byte_length()

def main():
    var s = String("text")
    var t = s
    t += "!"
    print(s, t, take(s))
