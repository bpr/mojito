# expect: not concrete outside parameter position
# The bare placeholder origin is concrete only in parameter position; a
# return annotation spelling it is not concrete (upstream: "'Pointer[Int, _]'
# is not concrete, use '[]' to bind missing parameters").
def f(p: Pointer[Int, _]) -> Pointer[Int, _]:
    return p

def main():
    print(1)
