-- t_device was the first device table and has been unused since the
-- space-member / trusted-peer model replaced it. Remove the empty shell.
DROP TABLE IF EXISTS t_device;
