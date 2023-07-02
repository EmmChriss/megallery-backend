# megallery-backend

<a name="readme-top"></a>

Framework for visualizing large numbers of images

<!-- TABLE OF CONTENTS -->
<details>
  <summary>Table of Contents</summary>
  <ol>
    <li>
      <a href="#about-the-project">About The Project</a>
      <ul>
        <li><a href="#built-with">Built With</a></li>
      </ul>
    </li>
    <li>
      <a href="#getting-started">Getting Started</a>
      <ul>
        <li><a href="#prerequisites">Prerequisites</a></li>
        <li><a href="#installation">Installation</a></li>
      </ul>
    </li>
    <li><a href="#usage">Usage</a></li>
    <li><a href="#contact">Contact</a></li>
  </ol>
</details>



<!-- ABOUT THE PROJECT -->
## About The Project

This project is the backend of Megallery, a framework for visualizing large numbers of images.

Such a project is interesting since I couldn't find a publicly available app that can display large numbers of images (>250k).

It also serves the purpose of helping you recognize patterns about your collections of images.

<p align="right">(<a href="#readme-top">back to top</a>)</p>



### Built With

* [Rust](https://www.rust-lang.org/)
* [Axum](https://github.com/tokio-rs/axum)

<p align="right">(<a href="#readme-top">back to top</a>)</p>



<!-- GETTING STARTED -->
## Getting Started

To get a local copy up and running follow these simple example steps.

### Prerequisites


* PostgreSQL
  ```sh
  docker-compose up megallery-db
  ```
  You're also going to need to set the `DATABASE_PORT` and `DATABASE_URL` env variables accordingly. There is dotenv support.
* Rust:

  You can get a Rust compiler installed using [rustup](https://rustup.rs/)

### Building

1. Clone the repo
   ```sh
   git clone https://github.com/EmmChriss/megallery-backend
   ```
2. Start the PostgreSQL instance
   ```sh
   docker-compose up megallery-db
   ```
3. Set up the environment
   ```sh
   cp .env.example .env 
   ```
3. Run using Cargo
   ```sh
   cargo run --release
   ```

<p align="right">(<a href="#readme-top">back to top</a>)</p>



<!-- USAGE EXAMPLES -->
## Usage

To use this backend, you're going to need the [frontend](https://github.com/EmmChriss/megallery-frontend) as well.

<p align="right">(<a href="#readme-top">back to top</a>)</p>



<!-- CONTACT -->
## Contact

Molnar Krisztian - emmchris@protonmail.com

Project Link: [https://github.com/EmmChriss/megallery-backend](https://github.com/EmmChriss/megallery-backend)

<p align="right">(<a href="#readme-top">back to top</a>)</p>