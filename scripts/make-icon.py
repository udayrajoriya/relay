from PIL import Image, ImageDraw
from pathlib import Path
dest = Path(__file__).resolve().parent.parent / 'src' / 'assets'
image = Image.new('RGBA', (1024, 1024), (0, 0, 0, 0))
draw = ImageDraw.Draw(image)
draw.rounded_rectangle((32, 32, 992, 992), radius=256, fill='#7061e8')
draw.polygon([(580,160),(264,572),(500,572),(432,864),(764,420),(524,420)], fill='white')
image.resize((256,256), Image.Resampling.LANCZOS).save(dest/'icon.png')
image.save(dest/'icon.ico', sizes=[(16,16),(24,24),(32,32),(48,48),(64,64),(128,128),(256,256)])
