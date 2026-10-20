# expect: unexpected keyword parameter 'a'
# Positional arguments bind before keywords, so `2` fills `a` and the
# keyword finds its slot already bound, as on the pin.
struct C[a: Int, b: Int]:
    def __init__(out self):
        pass


def main():
    var c = C[a=1, 2]()
    _ = c^
