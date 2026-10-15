# expect: '@inline' decorator takes exactly 1 argument, found 0
@inline
def twice(x: Int) -> Int:
    return x * 2

def main():
    print(twice(21))
