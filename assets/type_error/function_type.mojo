# A callable value must have exactly the annotated parameter and result types.
# expect: variable 'callback'
def wrong(value: String) -> Int:
    return 0

def main():
    var callback: def(Int) -> Int = wrong
