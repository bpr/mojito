# expect: call to raising function 'input' requires a surrounding 'try' block
# `input` raises at end of input, so a function that neither declares
# `raises` nor handles the error cannot call it.
def read_name() -> String:
    return input("name: ")

def main():
    try:
        print(read_name())
    except e:
        print(e)
