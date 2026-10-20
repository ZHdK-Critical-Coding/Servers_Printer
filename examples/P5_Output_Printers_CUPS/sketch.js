const printerConnection = 'ws://localhost:8070' // WebSocket-Adresse des Printer Servers (CUPS)

// Seitenoptionen für den Drucker (alle optional)
const pageOptions = {
  media: 'A4',            // 'A3', 'A4', 'A5', 'A6', 'Letter', 'Legal'
  orientation: 'auto',    // 'auto' (wie das Bild), 'portrait' oder 'landscape'
  fit: true,              // true: Bild füllt die Seite. false: Grösse aus dpi
  dpi: 96,                // nur mit fit: false. 96 = so gross wie auf dem Bildschirm
  margin: 10,             // weisser Rand in mm
  copies: 1,              // Anzahl Kopien
  dither: false,          // true: schwarz-weisse Punkte wie beim Nadeldrucker
}

let socket;                 // WebSocket-Objekt
let printerState = 'not ok' // Statusanzeige der Verbindung
let statusLabel             // HTML-Element für den Status (wird nicht mitgedruckt)
let seed = 1                // Zufallszahl für die Grafik

function communicateWithPrinter(sendData) {
  if (!socket || socket.readyState !== WebSocket.OPEN) {
    return Promise.reject('socket not open') // Keine Verbindung: sofort abbrechen
  }

  // Promise, das auf die Antwort vom Server wartet
  return new Promise((resolve, reject) => {
    // Alle Listener wieder entfernen, sobald eine Antwort (oder ein Fehler) da ist
    const cleanup = () => {
      socket.removeEventListener('message', handler)
      socket.removeEventListener('close', closeHandler)
    }
    // Event-Listener für einmalige Antwort
    const handler = (event) => {
      cleanup()
      if (event.data == 'ok') {
        resolve(event.data) // Server bestätigt: Auftrag ist beim Drucker
      } else {
        reject(event.data)  // z.B. 'busy' oder 'error: ...'
      }
    }
    // Verbindung bricht ab, bevor eine Antwort kommt
    const closeHandler = () => {
      cleanup()
      reject('socket closed')
    }
    socket.addEventListener('message', handler)
    socket.addEventListener('close', closeHandler)
    socket.send(sendData)
  })
}

// Canvas als PNG an den Drucker senden
async function printCanvas() {
  const blob = await new Promise(resolve => canvas.toBlob(resolve, 'image/png'))
  await communicateWithPrinter(blob)
}

function setup() {
  // A4-Proportionen. Mit pixelDensity(3) wird das PNG 3x so gross: schärferer Druck
  createCanvas(420, 594)
  pixelDensity(3)

  // WebSocket-Verbindung zum Printer Server
  socket = new WebSocket(printerConnection)

  socket.onopen = async () => {
    printerState = 'Connected to printer. Click to print.'
    try {
      await communicateWithPrinter(JSON.stringify(pageOptions)) // Seitenoptionen einmal setzen
    } catch (e) {
      printerState = `Options error: ${e}`
    }
  }
  socket.onerror = () => {
    printerState = 'WebSocket Error'
  }
  socket.onclose = () => {
    printerState = 'Connection closed'
  }

  statusLabel = createP(printerState)
}

function draw() {
  randomSeed(seed)
  background(255)
  noFill()
  stroke(0)

  // Linien, die von der Mitte nach aussen laufen
  for (let i = 0; i < 120; i++) {
    const angle = random(TWO_PI)
    const length = random(50, width)
    strokeWeight(random(0.5, 3))
    stroke(random(255), 0, random(255))
    line(width / 2, height / 2, width / 2 + cos(angle) * length, height / 2 + sin(angle) * length)
  }

  noStroke()
  fill(0)
  textSize(48)
  textAlign(CENTER)
  text(`Nr. ${seed}`, width / 2, height - 40)

  statusLabel.html(printerState)
}

// Klick auf das Canvas: drucken und dann neue Grafik
async function mousePressed() {
  if (mouseX < 0 || mouseY < 0 || mouseX > width || mouseY > height) return
  try {
    printerState = 'Printing...'
    await printCanvas()
    printerState = 'Printed. Click to print again.'
    seed++
  } catch (e) {
    printerState = `Printer error: ${e}`
  }
}
