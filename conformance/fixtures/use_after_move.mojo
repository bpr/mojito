@fieldwise_init
struct Token(ImplicitlyCopyable):
    var value: Int

def main():
    var first = Token(11)
    var second = first^
    print(first.value, second.value)
