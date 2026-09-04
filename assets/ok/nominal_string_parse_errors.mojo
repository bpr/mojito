# atol/atof failures raise with upstream's messages: bad characters, empty
# input, misplaced separators, out-of-range values, bad bases, and float
# shapes (first/last character, invalid characters).
def show_atol(s: String, base: Int = 10):
    try:
        print(atol(s, base))
    except e:
        print("error:", e)

def show_atof(s: String):
    try:
        print(atof(s))
    except e:
        print("error:", e)

def main():
    show_atol("017", 0)
    show_atol("abc")
    show_atol("")
    show_atol("12a")
    show_atol("1__0")
    show_atol("_1")
    show_atol("9223372036854775808")
    show_atol("5", 1)
    show_atol("- 5")
    show_atof("1_0.5")
    show_atof("abc")
    show_atof("")
    show_atof("1e")
    show_atof(".")
    try:
        print(Int(String("nope")))
    except e:
        print("caught:", e)
