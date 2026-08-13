# expect: has no method 'copy'
struct Plain:
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

def main():
    var p = Plain(1)
    var q = p.copy()
    print(q.x)
