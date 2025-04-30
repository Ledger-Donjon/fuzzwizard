-- Your SQL goes here

CREATE TABLE crashes (
	id SERIAL PRIMARY KEY, 
	input BYTEA NOT NULL,
	report BYTEA NOT NULL
);