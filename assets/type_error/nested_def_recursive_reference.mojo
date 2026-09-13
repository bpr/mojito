# expect: recursive reference to nested function 'walk'
# A function nested inside `walk` still names `walk` from within `walk`'s own
# body, which is recursion through a nested function; it belongs at file scope.
def depth(n: Int) -> Int:
    def walk(k: Int) -> Int:
        def step(j: Int) -> Int:
            if j == 0:
                return 0
            return 1 + walk(j - 1)
        return step(k)
    return walk(n)

def main():
    print(depth(3))
